// SPDX-License-Identifier: Apache-2.0
//! `X-Wenlan-Space` HTTP header extractor (dual-read for backward compat).
//!
//! Accepts both `X-Wenlan-Space: <name>` (new, preferred) and `X-Origin-Space: <name>` (legacy).
//! Prefers the new header; falls back to legacy if only that is present.
//! Most handlers use it as a fallback applied only when the request body omits the `space` field.
//! Repair preparation handlers instead require body/report scope to match a present header.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;

use crate::error::ServerError;

pub const HEADER_NAME: &str = "x-wenlan-space";
pub const HEADER_NAME_LEGACY: &str = "x-origin-space";

#[derive(Debug, Clone, Default)]
pub struct SpaceHeader(pub Option<String>);

impl<S> FromRequestParts<S> for SpaceHeader
where
    S: Send + Sync,
{
    type Rejection = ServerError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get(HEADER_NAME)
            .or_else(|| parts.headers.get(HEADER_NAME_LEGACY));
        let val = header
            .map(|value| {
                std::str::from_utf8(value.as_bytes())
                    .map_err(|_| ServerError::ValidationError("invalid Space header".to_string()))
            })
            .transpose()?
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        Ok(SpaceHeader(val))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;

    fn build_parts(headers: &[(&str, &str)]) -> Parts {
        let mut req = Request::builder();
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.body(()).unwrap().into_parts().0
    }

    #[tokio::test]
    async fn missing_header_yields_none() {
        let mut parts = build_parts(&[]);
        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn present_header_yields_some() {
        let mut parts = build_parts(&[("X-Origin-Space", "career")]);
        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(val.as_deref(), Some("career"));
    }

    #[tokio::test]
    async fn empty_header_yields_none() {
        let mut parts = build_parts(&[("X-Origin-Space", "   ")]);
        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(val, None);
    }

    #[tokio::test]
    async fn legacy_origin_header_still_read() {
        let mut parts = build_parts(&[("X-Origin-Space", "career")]);
        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(val.as_deref(), Some("career"));
    }

    #[tokio::test]
    async fn wenlan_header_read() {
        let mut parts = build_parts(&[("X-Wenlan-Space", "health")]);
        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(val.as_deref(), Some("health"));
    }

    #[tokio::test]
    async fn wenlan_header_accepts_valid_utf8_space_names() {
        let mut parts = build_parts(&[]);
        parts.headers.insert(
            HEADER_NAME,
            axum::http::HeaderValue::from_bytes("研究空間".as_bytes()).unwrap(),
        );

        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();

        assert_eq!(val.as_deref(), Some("研究空間"));
    }

    #[tokio::test]
    async fn wenlan_header_wins_when_both_names_are_present() {
        let mut parts = build_parts(&[
            ("X-Origin-Space", "legacy"),
            ("X-Wenlan-Space", "preferred"),
        ]);
        let SpaceHeader(val) = SpaceHeader::from_request_parts(&mut parts, &())
            .await
            .unwrap();
        assert_eq!(val.as_deref(), Some("preferred"));
    }

    #[tokio::test]
    async fn malformed_preferred_header_is_rejected_instead_of_falling_back() {
        let mut parts = build_parts(&[("X-Origin-Space", "legacy")]);
        parts.headers.insert(
            HEADER_NAME,
            axum::http::HeaderValue::from_bytes(&[0xff]).unwrap(),
        );

        let result = SpaceHeader::from_request_parts(&mut parts, &()).await;

        assert!(
            result.is_err(),
            "a malformed preferred selector must not become Global or use the legacy selector"
        );
    }
}
