use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::extract::Request;
use axum::http::{HeaderName, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use rmcp::model::{ClientJsonRpcMessage, ClientRequest};
use rmcp::transport::streamable_http_server::{
    session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
};
use tower_http::cors::CorsLayer;

use crate::auth;
use crate::client::{AgentIdentity, WenlanClient};
use crate::lock_state;
use crate::tools::{ToolProfile, TransportMode, WenlanMcpServer};

pub const QUERY_ONLY_AUTH_ERROR: &str =
    "Query-only tool profile requires bearer token authentication; --no-auth is not allowed.";

pub const QUERY_ONLY_SPACE_ERROR: &str =
    "Query-only tool profile requires a strict WENLAN_SPACE pin or --whole-library; WENLAN_DEFAULT_SPACE alone does not satisfy it.";

/// Reserved scope value for a connector that searches the whole library:
/// every Space plus everything not in a Space. Core refuses it as a Space name.
pub const WHOLE_LIBRARY_SPACE: &str = "*";

pub const WEB_WRITE_AUTH_ERROR: &str =
    "Web-write tool profile requires bearer token authentication; --no-auth is not allowed.";

pub const WEB_WRITE_SPACE_ERROR: &str =
    "Web-write tool profile requires a strict WENLAN_SPACE pin or --whole-library; WENLAN_DEFAULT_SPACE alone does not satisfy it.";

pub const WHOLE_LIBRARY_PROFILE_ERROR: &str =
    "--whole-library is only valid with --tool-profile query-only or web-write.";

pub const WHOLE_LIBRARY_PIN_CONFLICT_ERROR: &str =
    "--whole-library cannot be combined with a WENLAN_SPACE pin; choose one scope.";

pub const RESERVED_SPACE_PIN_ERROR: &str =
    "WENLAN_SPACE cannot be the reserved whole-library value \"*\"; use --whole-library instead.";

#[derive(Debug, Clone)]
pub struct ServeConfig {
    pub port: u16,
    pub host: String,
    pub origin_url: String,
    pub token: Option<String>,
    pub agent_name: String,
    /// Whether `agent_name` came from explicit configuration. Only an
    /// implicit transport default may yield to MCP `clientInfo.name`.
    pub agent_name_explicit: bool,
    pub user_id: Option<String>,
    pub allowed_origins: Vec<String>,
    /// Query-only: search the whole library instead of one pinned Space.
    pub whole_library: bool,
}

/// The profiles served to a web AI app through the relay: bearer-authenticated
/// and pinned to exactly one scope (a Space or the whole library).
pub fn is_connector_profile(tool_profile: ToolProfile) -> bool {
    matches!(tool_profile, ToolProfile::QueryOnly | ToolProfile::WebWrite)
}

/// The startup auth refusal for a connector profile.
pub fn connector_auth_error(tool_profile: ToolProfile) -> &'static str {
    match tool_profile {
        ToolProfile::WebWrite => WEB_WRITE_AUTH_ERROR,
        _ => QUERY_ONLY_AUTH_ERROR,
    }
}

/// Resolve the scope a connector reports in `/connector-info`: the strict Space
/// pin, or [`WHOLE_LIBRARY_SPACE`]. Exactly one must be chosen, and a pin can
/// never spell the reserved value, so a one-Space connector is never widened
/// into the whole library. Query-only and web-write share these rules.
pub fn connector_scope(
    tool_profile: ToolProfile,
    locked: Option<String>,
    whole_library: bool,
) -> anyhow::Result<String> {
    match (locked, whole_library) {
        (Some(_), true) => anyhow::bail!(WHOLE_LIBRARY_PIN_CONFLICT_ERROR),
        (Some(space), false) if space == WHOLE_LIBRARY_SPACE => {
            anyhow::bail!(RESERVED_SPACE_PIN_ERROR)
        }
        (Some(space), false) => Ok(space),
        (None, true) => Ok(WHOLE_LIBRARY_SPACE.to_string()),
        (None, false) if tool_profile == ToolProfile::WebWrite => {
            anyhow::bail!(WEB_WRITE_SPACE_ERROR)
        }
        (None, false) => anyhow::bail!(QUERY_ONLY_SPACE_ERROR),
    }
}

/// Resolve the scope a query-only connector reports in `/connector-info`.
pub fn query_only_scope(locked: Option<String>, whole_library: bool) -> anyhow::Result<String> {
    connector_scope(ToolProfile::QueryOnly, locked, whole_library)
}

async fn health() -> impl IntoResponse {
    axum::Json(serde_json::json!({
        "status": "ok",
        "server": "wenlan-mcp",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

pub async fn run_serve(config: ServeConfig) -> anyhow::Result<()> {
    run_serve_with_profile(config, ToolProfile::Standard).await
}

pub async fn run_serve_with_profile(
    config: ServeConfig,
    tool_profile: ToolProfile,
) -> anyhow::Result<()> {
    if is_connector_profile(tool_profile)
        && config
            .token
            .as_deref()
            .is_none_or(|token| token.trim().is_empty())
    {
        anyhow::bail!(connector_auth_error(tool_profile));
    }
    if config.whole_library && !is_connector_profile(tool_profile) {
        anyhow::bail!(WHOLE_LIBRARY_PROFILE_ERROR);
    }
    // A Space pin scopes retrieval to one Space; it is a data-scoping control,
    // not authentication or per-caller tenant isolation.
    let connector_space = if is_connector_profile(tool_profile) {
        Some(connector_scope(
            tool_profile,
            lock_state::locked_space(),
            config.whole_library,
        )?)
    } else {
        None
    };
    if config.whole_library {
        lock_state::clear_default_space();
    }

    let origin_url = config.origin_url.clone();
    let agent_name = config.agent_name.clone();
    let agent_name_explicit = config.agent_name_explicit;
    let user_id = config.user_id.clone();
    let token = config.token.clone();
    let allowed_origins = config.allowed_origins.clone();

    // Keep the SDK's loopback Host allowlist for unauthenticated servers:
    // loopback binding and CORS do not stop same-origin DNS rebinding.
    // Authenticated tunnels forward a public hostname in Host, so only that
    // mode disables Host filtering; the outer bearer gate still protects it.
    let mut mcp_config = StreamableHttpServerConfig::default();
    if token.is_some() {
        mcp_config = mcp_config.disable_allowed_hosts();
    }
    if is_connector_profile(tool_profile) {
        // Let the SDK frame heartbeats between complete SSE events. Frequent
        // writes also expose a disconnected HTTP reader to the relay runtime.
        mcp_config = mcp_config.with_sse_keep_alive(Some(std::time::Duration::from_secs(1)));
    }

    let mcp_service = StreamableHttpService::new(
        move || {
            let identity = AgentIdentity::new(agent_name.clone(), agent_name_explicit);
            Ok(WenlanMcpServer::new_with_identity(
                WenlanClient::new(origin_url.clone()),
                TransportMode::Http,
                identity,
                user_id.clone(),
            )
            .with_tool_profile(tool_profile))
        },
        Arc::new(LocalSessionManager::default()),
        mcp_config,
    );

    let cors = build_cors_layer(&config.allowed_origins);

    let mut router = Router::new()
        .nest_service("/mcp", mcp_service)
        .route("/health", get(health));

    if let Some(space) = connector_space {
        let profile_name = tool_profile.wire_name();
        // Enrollment reads this only through the same bearer gate as MCP.
        // Never put the Space pin in the public health response.
        router = router.route(
            "/connector-info",
            get(move || {
                let space = space.clone();
                async move {
                    (
                        [(http::header::CACHE_CONTROL, "no-store")],
                        axum::Json(serde_json::json!({
                            "contract_version": 1,
                            "server": "wenlan-mcp",
                            "tool_profile": profile_name,
                            "space": space,
                            "authentication": "bearer",
                        })),
                    )
                }
            }),
        );
    }
    // Pre-allocation initialization guard for every HTTP tool profile.
    // Auth remains outermost (applied later); health/GET/existing-session bypass inside.
    router = router.layer(middleware::from_fn(validate_initialization));
    router = router.layer(cors);

    if let Some(ref expected_token) = token {
        let token_for_middleware = expected_token.clone();
        let origins_for_middleware = allowed_origins.clone();
        router = router.layer(middleware::from_fn(move |req: Request, next: Next| {
            let token = token_for_middleware.clone();
            let origins = origins_for_middleware.clone();
            async move { auth_and_origin_middleware(req, next, &token, &origins).await }
        }));
    }

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("wenlan-mcp HTTP server listening on {}", addr);

    if token.is_some() {
        tracing::info!("Bearer token authentication enabled");
    } else {
        tracing::warn!("Running without authentication — only safe on loopback");
    }

    let shutdown = async {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let ctrl_c = tokio::signal::ctrl_c();
            let mut sigterm =
                signal(SignalKind::terminate()).expect("failed to register SIGTERM handler");
            tokio::select! {
                _ = ctrl_c => {},
                _ = sigterm.recv() => {},
            }
        }
        #[cfg(not(unix))]
        {
            tokio::signal::ctrl_c().await.ok();
        }
        tracing::info!("Shutting down wenlan-mcp HTTP server");
    };

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown)
        .await?;

    Ok(())
}

// Reject invalid first messages before SDK session allocation on every HTTP
// tool profile. The SDK also cleans up failed initialization, while this guard
// retains Wenlan's bounded body size, read timeout, and initialization contract.
async fn validate_initialization(req: Request, next: Next) -> axum::response::Response {
    let is_mcp = req.uri().path() == "/mcp" || req.uri().path().starts_with("/mcp/");
    let has_session = req
        .headers()
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
        .is_some();
    if !is_mcp || req.method() != Method::POST || has_session {
        return next.run(req).await;
    }
    let (parts, body) = req.into_parts();
    let bytes = match tokio::time::timeout(Duration::from_secs(5), to_bytes(body, 64 * 1024)).await
    {
        Ok(Ok(bytes)) => bytes,
        Ok(Err(_)) => {
            return (
                StatusCode::PAYLOAD_TOO_LARGE,
                "MCP initialization too large",
            )
                .into_response()
        }
        Err(_) => {
            return (StatusCode::REQUEST_TIMEOUT, "MCP initialization timed out").into_response()
        }
    };
    let valid = match serde_json::from_slice::<ClientJsonRpcMessage>(&bytes) {
        Ok(ClientJsonRpcMessage::Request(rpc)) => {
            matches!(rpc.request, ClientRequest::InitializeRequest(_))
        }
        _ => false,
    };
    if !valid {
        return (StatusCode::BAD_REQUEST, "Initialize request required").into_response();
    }
    next.run(Request::from_parts(parts, Body::from(bytes)))
        .await
}

#[cfg(test)]
mod tests;

fn build_cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers([
            http::header::AUTHORIZATION,
            http::header::CONTENT_TYPE,
            http::header::ACCEPT,
            HeaderName::from_static("mcp-session-id"),
            HeaderName::from_static("mcp-protocol-version"),
        ]);

    if allowed_origins.iter().any(|o| o == "*") {
        cors.allow_origin(tower_http::cors::Any)
    } else {
        let origins: Vec<http::HeaderValue> = allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        cors.allow_origin(origins)
    }
}

/// Auth middleware: bearer token first (401), then Origin header (403).
async fn auth_and_origin_middleware(
    req: Request,
    next: Next,
    expected_token: &str,
    allowed_origins: &[String],
) -> axum::response::Response {
    let is_preflight = req.method() == Method::OPTIONS;
    let is_health = req.uri().path() == "/health";
    if is_preflight || is_health {
        return next.run(req).await;
    }

    // 1. Validate bearer token FIRST
    let auth_header = req.headers().get(http::header::AUTHORIZATION);
    match auth_header {
        Some(value) => {
            let value_str = value.to_str().unwrap_or("");
            match value_str.strip_prefix("Bearer ") {
                Some(provided) if auth::verify_token(provided, expected_token) => {}
                _ => return (StatusCode::UNAUTHORIZED, "Invalid bearer token").into_response(),
            }
        }
        None => return (StatusCode::UNAUTHORIZED, "Authorization header required").into_response(),
    }

    // 2. Validate Origin header AFTER auth
    if let Some(origin) = req.headers().get(http::header::ORIGIN) {
        if let Ok(origin_str) = origin.to_str() {
            if !auth::is_origin_allowed(origin_str, allowed_origins) {
                return (StatusCode::FORBIDDEN, "Origin not allowed").into_response();
            }
        }
    }

    next.run(req).await
}
