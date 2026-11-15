// SPDX-License-Identifier: Apache-2.0
//! Embedded production browser UI for the personal loopback listener.

use axum::{
    body::Body,
    extract::Request,
    http::{header, HeaderValue, Method, StatusCode},
    response::Response,
};

pub(crate) struct EmbeddedAsset {
    path: &'static str,
    content_type: &'static str,
    bytes: &'static [u8],
}
include!(concat!(env!("OUT_DIR"), "/web_assets.rs"));

const FALLBACK: &str = "<!doctype html><meta charset=utf-8><title>Wenlan</title><h1>Wenlan web UI was not built in this binary</h1><p>From the source checkout, run <code>pnpm build:web</code> before building wenlan-server.</p>";

pub(crate) async fn handle(request: Request) -> Response {
    let path = request.uri().path();
    let is_asset = WEB_ASSETS
        .iter()
        .any(|asset| asset.path == path.trim_start_matches('/'));
    let accepts_html = request
        .headers()
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("text/html") || value.contains("*/*"));
    if path != "/"
        && !is_asset
        && !path.starts_with("/api")
        && !path.starts_with("/mcp")
        && !path.starts_with("/health")
        && !accepts_html
    {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    response_for(
        request.method(),
        request.uri().path(),
        matches!(
            crate::security::bind_scope_from_env(),
            crate::security::BindScope::Unset | crate::security::BindScope::Loopback
        ),
        WEB_ASSETS,
    )
}

fn response_for(
    method: &Method,
    path: &str,
    loopback: bool,
    assets: &'static [EmbeddedAsset],
) -> Response {
    if !loopback || *method != Method::GET {
        return plain(StatusCode::NOT_FOUND, "not found");
    }
    if path == "/api"
        || path.starts_with("/api/")
        || path == "/mcp"
        || path.starts_with("/mcp/")
        || path == "/health"
        || path.starts_with("/health/")
    {
        return response(
            StatusCode::NOT_FOUND,
            "application/json",
            b"{\"error\":\"not found\"}",
            "no-store",
        );
    }
    let wanted = path.trim_start_matches('/');
    if !wanted.is_empty() {
        if let Some(asset) = assets.iter().find(|asset| asset.path == wanted) {
            let immutable = wanted.starts_with("assets/")
                && wanted
                    .rsplit('/')
                    .next()
                    .is_some_and(|name| name.matches('-').count() > 0);
            return response(
                StatusCode::OK,
                asset.content_type,
                asset.bytes,
                if immutable {
                    "public, max-age=31536000, immutable"
                } else {
                    "no-cache"
                },
            );
        }
    }
    if let Some(index) = assets.iter().find(|asset| asset.path == "index.html") {
        return response(StatusCode::OK, index.content_type, index.bytes, "no-store");
    }
    missing_bundle_response()
}

fn missing_bundle_response() -> Response {
    response(
        StatusCode::SERVICE_UNAVAILABLE,
        "text/html; charset=utf-8",
        FALLBACK.as_bytes(),
        "no-store",
    )
}

fn plain(status: StatusCode, body: &'static str) -> Response {
    response(
        status,
        "text/plain; charset=utf-8",
        body.as_bytes(),
        "no-store",
    )
}
fn response(
    status: StatusCode,
    content_type: &'static str,
    bytes: &'static [u8],
    cache: &'static str,
) -> Response {
    let mut result = Response::new(Body::from(bytes));
    *result.status_mut() = status;
    let headers = result.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(header::CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"));
    headers.insert(
        "x-content-type-options",
        HeaderValue::from_static("nosniff"),
    );
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{middleware, routing::get, Router};
    use tower::ServiceExt;

    static TEST_ASSETS: &[EmbeddedAsset] = &[
        EmbeddedAsset {
            path: "index.html",
            content_type: "text/html; charset=utf-8",
            bytes: b"<!doctype html><title>test bundle</title>",
        },
        EmbeddedAsset {
            path: "assets/index-deadbeef.js",
            content_type: "text/javascript; charset=utf-8",
            bytes: b"console.log('test bundle')",
        },
    ];

    #[test]
    fn protected_paths_never_receive_the_spa() {
        let response = response_for(&Method::GET, "/api/not-a-route", true, TEST_ASSETS);
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    }

    #[test]
    fn missing_bundle_has_an_actionable_fallback_and_external_bind_has_none() {
        let fallback = missing_bundle_response();
        assert_eq!(fallback.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(fallback.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            response_for(&Method::GET, "/", false, TEST_ASSETS).status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            response_for(&Method::GET, "/", true, &[]).status(),
            StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[tokio::test]
    async fn embedded_index_spa_and_hashed_asset_headers() {
        let index = response_for(&Method::GET, "/", true, TEST_ASSETS);
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(
            index.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(index.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            axum::body::to_bytes(index.into_body(), usize::MAX)
                .await
                .unwrap(),
            &b"<!doctype html><title>test bundle</title>"[..]
        );

        let spa = response_for(&Method::GET, "/some/deep/path", true, TEST_ASSETS);
        assert_eq!(spa.status(), StatusCode::OK);
        assert_eq!(
            spa.headers()[header::CONTENT_TYPE],
            "text/html; charset=utf-8"
        );
        assert_eq!(spa.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(
            axum::body::to_bytes(spa.into_body(), usize::MAX)
                .await
                .unwrap(),
            &b"<!doctype html><title>test bundle</title>"[..]
        );

        let asset = response_for(&Method::GET, "/assets/index-deadbeef.js", true, TEST_ASSETS);
        assert_eq!(asset.status(), StatusCode::OK);
        assert_eq!(
            asset.headers()[header::CONTENT_TYPE],
            "text/javascript; charset=utf-8"
        );
        assert_eq!(
            asset.headers()[header::CACHE_CONTROL],
            "public, max-age=31536000, immutable"
        );
        let csp = asset.headers()[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap();
        assert!(csp.contains("connect-src 'self'"));
        assert!(!csp.contains("ws:"));
        assert!(!csp.contains("wss:"));
        assert_eq!(
            axum::body::to_bytes(asset.into_body(), usize::MAX)
                .await
                .unwrap(),
            &b"console.log('test bundle')"[..]
        );
    }

    #[tokio::test]
    async fn cross_origin_guard_refuses_web_and_api() {
        let app = Router::new()
            .route("/api/probe", get(|| async { "secret" }))
            .fallback(handle)
            .layer(middleware::from_fn(crate::security::guard_local_only));
        for path in ["/", "/api/probe"] {
            let request = Request::builder()
                .uri(path)
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
            let request = Request::builder()
                .uri(path)
                .header("sec-fetch-site", "cross-site")
                .body(Body::empty())
                .unwrap();
            assert_eq!(
                app.clone().oneshot(request).await.unwrap().status(),
                StatusCode::FORBIDDEN
            );
        }
    }
}
