// SPDX-License-Identifier: AGPL-3.0-only
//! The desktop half of the relay's frozen v1 reverse wire protocol.
//!
//! This module owns framing and validation only. It does not authenticate a
//! peer, choose an endpoint, or manage the transport that carries a frame.

use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::de::{self, Deserializer, Error as DeError, Visitor};
use serde::ser::{Error as SerError, SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;

pub const MAX_WIRE_BYTES: usize = 96 * 1024;
pub const MAX_REQUEST_BODY_BYTES: usize = 64 * 1024;
pub const MAX_CHUNK_BODY_BYTES: usize = 16 * 1024;
pub const MAX_ID_BYTES: usize = 64;
pub const MAX_HEADER_VALUE_BYTES: usize = 2048;
pub const MAX_SAFE_SEQUENCE: u64 = 9_007_199_254_740_991;

const MAX_REQUEST_BODY_BASE64_BYTES: usize = MAX_REQUEST_BODY_BYTES.div_ceil(3) * 4;
const MAX_CHUNK_BODY_BASE64_BYTES: usize = MAX_CHUNK_BODY_BYTES.div_ceil(3) * 4;
const INVALID_MESSAGE: &str = "Invalid reverse frame";

/// Header names and values as they appear on the wire.
pub type HeaderMap = BTreeMap<String, String>;

/// A deliberately opaque protocol error. In particular, it never includes a
/// malformed frame, a header, or a body in either its display or debug form.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ReverseProtocolError;

impl fmt::Debug for ReverseProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("ReverseProtocolError")
    }
}

impl fmt::Display for ReverseProtocolError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(INVALID_MESSAGE)
    }
}

impl std::error::Error for ReverseProtocolError {}

/// The six v1 messages. The fixed `v: 1` discriminator is added by the wire
/// serializer and is not caller-controlled.
#[derive(Clone, PartialEq, Eq)]
pub enum Frame {
    Request {
        id: String,
        path: String,
        method: String,
        headers: HeaderMap,
        body: String,
    },
    Response {
        id: String,
        status: u16,
        headers: HeaderMap,
    },
    Chunk {
        id: String,
        seq: u64,
        body: String,
    },
    End {
        id: String,
    },
    Cancel {
        id: String,
    },
    Credit {
        id: String,
        seq: u64,
    },
}

impl fmt::Debug for Frame {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Frame([redacted])")
    }
}

/// JSON numbers are parsed by JavaScript as IEEE-754 numbers. These typed
/// serde fields accept the same integral spellings (`1`, `1.0`, and `1e0`)
/// while enforcing the relevant safe upper bound before conversion.
#[derive(Clone, Copy, PartialEq, Eq)]
struct WireInteger<const MAX: u64>(u64);

impl<const MAX: u64> Serialize for WireInteger<MAX> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_u64(self.0)
    }
}

impl<'de, const MAX: u64> Deserialize<'de> for WireInteger<MAX> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct IntegerVisitor<const MAX: u64>;

        impl<'de, const MAX: u64> Visitor<'de> for IntegerVisitor<MAX> {
            type Value = WireInteger<MAX>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(formatter, "an integer from 0 through {MAX}")
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value <= MAX {
                    Ok(WireInteger(value))
                } else {
                    Err(E::custom(INVALID_MESSAGE))
                }
            }

            fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value >= 0 {
                    self.visit_u64(value as u64)
                } else {
                    Err(E::custom(INVALID_MESSAGE))
                }
            }

            fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value <= MAX as u128 {
                    Ok(WireInteger(value as u64))
                } else {
                    Err(E::custom(INVALID_MESSAGE))
                }
            }

            fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value >= 0 {
                    self.visit_u128(value as u128)
                } else {
                    Err(E::custom(INVALID_MESSAGE))
                }
            }

            fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.is_finite() && value >= 0.0 && value.fract() == 0.0 {
                    let integer = value as u64;
                    if integer <= MAX && integer as f64 == value {
                        return Ok(WireInteger(integer));
                    }
                }
                Err(E::custom(INVALID_MESSAGE))
            }
        }

        deserializer.deserialize_any(IntegerVisitor::<MAX>)
    }
}

type WireVersion = WireInteger<1>;
type WireSequence = WireInteger<MAX_SAFE_SEQUENCE>;
type WireStatus = WireInteger<599>;

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
enum IncomingFrame {
    Request {
        v: WireVersion,
        id: String,
        path: String,
        method: String,
        headers: HeaderMap,
        body: String,
    },
    Response {
        v: WireVersion,
        id: String,
        status: WireStatus,
        headers: HeaderMap,
    },
    Chunk {
        v: WireVersion,
        id: String,
        seq: WireSequence,
        body: String,
    },
    End {
        v: WireVersion,
        id: String,
    },
    Cancel {
        v: WireVersion,
        id: String,
    },
    Credit {
        v: WireVersion,
        id: String,
        seq: WireSequence,
    },
}

/// Encode bytes as canonical padded standard base64, subject to the request
/// body bound shared by request and chunk frames.
pub fn encode_body(bytes: &[u8]) -> Result<String, ReverseProtocolError> {
    if bytes.len() > MAX_REQUEST_BODY_BYTES {
        return Err(ReverseProtocolError);
    }
    Ok(STANDARD.encode(bytes))
}

/// Decode only canonical padded standard base64. Re-encoding the decoded bits
/// rejects non-zero trailing padding bits that permissive decoders normalize.
pub fn decode_body(value: &str) -> Result<Vec<u8>, ReverseProtocolError> {
    if value.len() > MAX_REQUEST_BODY_BASE64_BYTES || !valid_base64_shape(value) {
        return Err(ReverseProtocolError);
    }
    let bytes = STANDARD
        .decode(value.as_bytes())
        .map_err(|_| ReverseProtocolError)?;
    if encode_body(&bytes)?.as_str() != value {
        return Err(ReverseProtocolError);
    }
    Ok(bytes)
}

pub fn encode_frame(frame: &Frame) -> Result<String, ReverseProtocolError> {
    let text = serde_json::to_string(frame).map_err(|_| ReverseProtocolError)?;
    if text.len() > MAX_WIRE_BYTES {
        return Err(ReverseProtocolError);
    }
    Ok(text)
}

pub fn decode_frame(text: &str) -> Result<Frame, ReverseProtocolError> {
    if text.len() > MAX_WIRE_BYTES {
        return Err(ReverseProtocolError);
    }
    serde_json::from_str(text).map_err(|_| ReverseProtocolError)
}

impl Serialize for Frame {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        validate_frame(self).map_err(|_| S::Error::custom(INVALID_MESSAGE))?;

        match self {
            Frame::Request {
                id,
                path,
                method,
                headers,
                body,
            } => {
                let mut map = serializer.serialize_map(Some(7))?;
                map.serialize_entry("v", &1u8)?;
                map.serialize_entry("type", "request")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("path", path)?;
                map.serialize_entry("method", method)?;
                map.serialize_entry("headers", headers)?;
                map.serialize_entry("body", body)?;
                map.end()
            }
            Frame::Response {
                id,
                status,
                headers,
            } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("v", &1u8)?;
                map.serialize_entry("type", "response")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("status", status)?;
                map.serialize_entry("headers", headers)?;
                map.end()
            }
            Frame::Chunk { id, seq, body } => {
                let mut map = serializer.serialize_map(Some(5))?;
                map.serialize_entry("v", &1u8)?;
                map.serialize_entry("type", "chunk")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("seq", seq)?;
                map.serialize_entry("body", body)?;
                map.end()
            }
            Frame::End { id } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("v", &1u8)?;
                map.serialize_entry("type", "end")?;
                map.serialize_entry("id", id)?;
                map.end()
            }
            Frame::Cancel { id } => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("v", &1u8)?;
                map.serialize_entry("type", "cancel")?;
                map.serialize_entry("id", id)?;
                map.end()
            }
            Frame::Credit { id, seq } => {
                let mut map = serializer.serialize_map(Some(4))?;
                map.serialize_entry("v", &1u8)?;
                map.serialize_entry("type", "credit")?;
                map.serialize_entry("id", id)?;
                map.serialize_entry("seq", seq)?;
                map.end()
            }
        }
    }
}

impl<'de> Deserialize<'de> for Frame {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = IncomingFrame::deserialize(deserializer)
            .map_err(|_| D::Error::custom(INVALID_MESSAGE))?;
        from_wire(wire).map_err(|_| D::Error::custom(INVALID_MESSAGE))
    }
}

fn from_wire(wire: IncomingFrame) -> Result<Frame, ReverseProtocolError> {
    let frame = match wire {
        IncomingFrame::Request {
            v,
            id,
            path,
            method,
            headers,
            body,
        } => {
            if v.0 != 1 {
                return Err(ReverseProtocolError);
            }
            Frame::Request {
                id,
                path,
                method,
                headers,
                body,
            }
        }
        IncomingFrame::Response {
            v,
            id,
            status,
            headers,
        } => {
            if v.0 != 1 {
                return Err(ReverseProtocolError);
            }
            Frame::Response {
                id,
                status: status.0 as u16,
                headers,
            }
        }
        IncomingFrame::Chunk { v, id, seq, body } => {
            if v.0 != 1 {
                return Err(ReverseProtocolError);
            }
            Frame::Chunk {
                id,
                seq: seq.0,
                body,
            }
        }
        IncomingFrame::End { v, id } => {
            if v.0 != 1 {
                return Err(ReverseProtocolError);
            }
            Frame::End { id }
        }
        IncomingFrame::Cancel { v, id } => {
            if v.0 != 1 {
                return Err(ReverseProtocolError);
            }
            Frame::Cancel { id }
        }
        IncomingFrame::Credit { v, id, seq } => {
            if v.0 != 1 {
                return Err(ReverseProtocolError);
            }
            Frame::Credit { id, seq: seq.0 }
        }
    };
    validate_frame(&frame)?;
    Ok(frame)
}

fn validate_frame(frame: &Frame) -> Result<(), ReverseProtocolError> {
    match frame {
        Frame::Request {
            id,
            path,
            method,
            headers,
            body,
        } => {
            validate_id(id)?;
            if path != "/mcp" && path != "/connector-info" {
                return Err(ReverseProtocolError);
            }
            if method != "GET" && method != "POST" && method != "DELETE" {
                return Err(ReverseProtocolError);
            }
            if path == "/connector-info" && method != "GET" {
                return Err(ReverseProtocolError);
            }
            validate_headers(headers, true)?;
            if (method == "GET" || method == "DELETE") && !body.is_empty() {
                return Err(ReverseProtocolError);
            }
            decode_body(body)?;
        }
        Frame::Response {
            id,
            status,
            headers,
        } => {
            validate_id(id)?;
            if !(*status >= 200 && *status <= 599) {
                return Err(ReverseProtocolError);
            }
            validate_headers(headers, false)?;
        }
        Frame::Chunk { id, seq, body } => {
            validate_id(id)?;
            validate_sequence(*seq)?;
            if body.len() > MAX_CHUNK_BODY_BASE64_BYTES {
                return Err(ReverseProtocolError);
            }
            let decoded = decode_body(body)?;
            if decoded.is_empty() || decoded.len() > MAX_CHUNK_BODY_BYTES {
                return Err(ReverseProtocolError);
            }
        }
        Frame::End { id } | Frame::Cancel { id } => validate_id(id)?,
        Frame::Credit { id, seq } => {
            validate_id(id)?;
            validate_sequence(*seq)?;
        }
    }
    Ok(())
}

fn validate_sequence(value: u64) -> Result<(), ReverseProtocolError> {
    if value <= MAX_SAFE_SEQUENCE {
        Ok(())
    } else {
        Err(ReverseProtocolError)
    }
}

fn validate_id(value: &str) -> Result<(), ReverseProtocolError> {
    if value.is_empty()
        || value.len() > MAX_ID_BYTES
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        Err(ReverseProtocolError)
    } else {
        Ok(())
    }
}

fn validate_headers(headers: &HeaderMap, request: bool) -> Result<(), ReverseProtocolError> {
    for (name, value) in headers {
        let allowed = if request {
            matches!(
                name.as_str(),
                "accept"
                    | "content-type"
                    | "mcp-session-id"
                    | "mcp-protocol-version"
                    | "last-event-id"
                    | "authorization"
                    | "x-wenlan-access"
            )
        } else {
            matches!(
                name.as_str(),
                "content-type" | "mcp-session-id" | "mcp-protocol-version"
            )
        };
        if !allowed
            || value.len() > MAX_HEADER_VALUE_BYTES
            || !value
                .bytes()
                .all(|byte| byte == b'\t' || (0x20..=0x7e).contains(&byte))
        {
            return Err(ReverseProtocolError);
        }
        if request && name == "authorization" && !valid_authorization(value) {
            return Err(ReverseProtocolError);
        }
        // The relay's note of which local server a request is for. It carries
        // no credential, and anything but these two words is not the contract.
        if request && name == "x-wenlan-access" && !matches!(value.as_str(), "query" | "write") {
            return Err(ReverseProtocolError);
        }
    }
    Ok(())
}

fn valid_authorization(value: &str) -> bool {
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    (32..=128).contains(&token.len())
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn valid_base64_shape(value: &str) -> bool {
    let bytes = value.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return false;
    }

    let padding = bytes.iter().rev().take_while(|&&byte| byte == b'=').count();
    if padding > 2 {
        return false;
    }
    let content_len = bytes.len() - padding;
    let expected_content_mod = match padding {
        0 => 0,
        1 => 3,
        2 => 2,
        _ => unreachable!(),
    };
    content_len % 4 == expected_content_mod
        && bytes[..content_len].iter().copied().all(is_base64_alphabet)
}

fn is_base64_alphabet(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'+' || byte == b'/'
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn request(body: &str) -> Frame {
        Frame::Request {
            id: "request-1".into(),
            path: "/mcp".into(),
            method: "POST".into(),
            headers: HeaderMap::new(),
            body: body.into(),
        }
    }

    fn response() -> Frame {
        Frame::Response {
            id: "response-1".into(),
            status: 200,
            headers: [("content-type".into(), "application/json".into())]
                .into_iter()
                .collect(),
        }
    }

    fn chunk(body: &str) -> Frame {
        Frame::Chunk {
            id: "chunk-1".into(),
            seq: 0,
            body: body.into(),
        }
    }

    fn assert_invalid(text: &str) {
        let error = decode_frame(text).expect_err("expected invalid frame");
        assert_eq!(error.to_string(), INVALID_MESSAGE);
        assert_eq!(format!("{error:?}"), "ReverseProtocolError");
    }

    fn keys(text: &str) -> Vec<String> {
        serde_json::from_str::<Value>(text)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect()
    }

    #[derive(Deserialize)]
    struct ReverseProtocolCorpus {
        valid: Vec<Value>,
        invalid: Vec<Value>,
    }

    #[test]
    fn shared_typescript_corpus_matches_rust_codec() {
        let corpus: ReverseProtocolCorpus = serde_json::from_str(include_str!(
            "../../../relay/tests/fixtures/reverse-protocol-corpus.json"
        ))
        .unwrap();

        for value in corpus.valid {
            let text = serde_json::to_string(&value).unwrap();
            let frame = decode_frame(&text).expect("shared valid frame should decode");
            let encoded = encode_frame(&frame).expect("decoded valid frame should encode");
            assert_eq!(serde_json::from_str::<Value>(&encoded).unwrap(), value);
        }
        for value in corpus.invalid {
            let text = serde_json::to_string(&value).unwrap();
            assert!(
                decode_frame(&text).is_err(),
                "shared invalid frame decoded: {text}"
            );
        }
    }

    #[test]
    fn body_helpers_round_trip_and_enforce_request_bound() {
        let bytes = "繁體中文 and emoji: 🧠".as_bytes();
        assert_eq!(decode_body(&encode_body(bytes).unwrap()).unwrap(), bytes);
        assert_eq!(
            decode_body(&encode_body(&[]).unwrap()).unwrap(),
            Vec::<u8>::new()
        );

        let max = vec![0u8; MAX_REQUEST_BODY_BYTES];
        assert_eq!(
            decode_body(&encode_body(&max).unwrap()).unwrap().len(),
            max.len()
        );
        assert!(encode_body(&vec![0u8; MAX_REQUEST_BODY_BYTES + 1]).is_err());
    }

    #[test]
    fn every_frame_variant_round_trips() {
        let mut authorized = HeaderMap::new();
        authorized.insert("authorization".into(), format!("Bearer {}", "t".repeat(32)));
        let frames = vec![
            Frame::Request {
                id: "request-1".into(),
                path: "/mcp".into(),
                method: "POST".into(),
                headers: authorized,
                body: encode_body(br#"{"jsonrpc":"2.0"}"#).unwrap(),
            },
            Frame::Request {
                id: "info".into(),
                path: "/connector-info".into(),
                method: "GET".into(),
                headers: HeaderMap::new(),
                body: String::new(),
            },
            Frame::Request {
                id: "get".into(),
                path: "/mcp".into(),
                method: "GET".into(),
                headers: HeaderMap::new(),
                body: String::new(),
            },
            Frame::Request {
                id: "delete".into(),
                path: "/mcp".into(),
                method: "DELETE".into(),
                headers: HeaderMap::new(),
                body: String::new(),
            },
            Frame::Response {
                id: "response-1".into(),
                status: 200,
                headers: [
                    ("content-type".into(), "text/event-stream".into()),
                    ("mcp-session-id".into(), "session-1".into()),
                ]
                .into_iter()
                .collect(),
            },
            Frame::Response {
                id: "no-content".into(),
                status: 204,
                headers: HeaderMap::new(),
            },
            chunk(&encode_body(&[0, 1, 2, 255]).unwrap()),
            Frame::End { id: "end-1".into() },
            Frame::Cancel {
                id: "cancel-1".into(),
            },
            Frame::Credit {
                id: "credit-1".into(),
                seq: MAX_SAFE_SEQUENCE,
            },
        ];

        for frame in frames {
            assert_eq!(decode_frame(&encode_frame(&frame).unwrap()).unwrap(), frame);
        }
    }

    #[test]
    fn request_and_chunk_body_bounds_are_enforced() {
        let max_request = encode_body(&vec![0u8; MAX_REQUEST_BODY_BYTES]).unwrap();
        assert_eq!(
            decode_frame(&encode_frame(&request(&max_request)).unwrap()).unwrap(),
            request(&max_request)
        );

        for method in ["GET", "DELETE"] {
            let frame = Frame::Request {
                id: "id".into(),
                path: "/mcp".into(),
                method: method.into(),
                headers: HeaderMap::new(),
                body: String::new(),
            };
            assert_eq!(decode_frame(&encode_frame(&frame).unwrap()).unwrap(), frame);
            let invalid = Frame::Request {
                id: "id".into(),
                path: "/mcp".into(),
                method: method.into(),
                headers: HeaderMap::new(),
                body: encode_body(&[1]).unwrap(),
            };
            assert!(encode_frame(&invalid).is_err());
        }

        let max_chunk = encode_body(&vec![0u8; MAX_CHUNK_BODY_BYTES]).unwrap();
        let max_chunk_frame = chunk(&max_chunk);
        assert_eq!(
            decode_frame(&encode_frame(&max_chunk_frame).unwrap()).unwrap(),
            max_chunk_frame
        );
        assert!(encode_frame(&chunk("")).is_err());
        let too_large_chunk = encode_body(&vec![0u8; MAX_CHUNK_BODY_BYTES + 1]).unwrap();
        assert!(encode_frame(&chunk(&too_large_chunk)).is_err());
    }

    #[test]
    fn headers_enforce_names_values_and_authorization() {
        let max = "x".repeat(MAX_HEADER_VALUE_BYTES);
        let mut headers = HeaderMap::new();
        headers.insert("accept".into(), max.clone());
        let frame = Frame::Request {
            id: "request-1".into(),
            path: "/mcp".into(),
            method: "POST".into(),
            headers,
            body: String::new(),
        };
        assert_eq!(decode_frame(&encode_frame(&frame).unwrap()).unwrap(), frame);

        for value in [
            format!("{}x", max),
            "line\nfeed".into(),
            "carriage\rreturn".into(),
            "nul\0byte".into(),
            "backspace\x08".into(),
            "delete\x7f".into(),
            "non-ascii: é".into(),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert("accept".into(), value);
            assert!(encode_frame(&Frame::Request {
                id: "request-1".into(),
                path: "/mcp".into(),
                method: "POST".into(),
                headers,
                body: String::new(),
            })
            .is_err());
        }

        let mut headers = HeaderMap::new();
        headers.insert("authorization".into(), "Bearer short".into());
        assert!(encode_frame(&Frame::Request {
            id: "request-1".into(),
            path: "/mcp".into(),
            method: "POST".into(),
            headers,
            body: String::new(),
        })
        .is_err());

        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization".into(),
            format!("Bearer {}", "a".repeat(128)),
        );
        assert!(encode_frame(&Frame::Request {
            id: "request-1".into(),
            path: "/mcp".into(),
            method: "POST".into(),
            headers,
            body: String::new(),
        })
        .is_ok());
    }

    #[test]
    fn access_header_is_a_request_only_note_of_query_or_write() {
        let with_access = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert("x-wenlan-access".into(), value.into());
            headers
        };
        for value in ["write", "query"] {
            let frame = Frame::Request {
                id: "request-1".into(),
                path: "/mcp".into(),
                method: "POST".into(),
                headers: with_access(value),
                body: String::new(),
            };
            assert_eq!(decode_frame(&encode_frame(&frame).unwrap()).unwrap(), frame);
        }
        for value in ["", "WRITE", "admin", "write "] {
            assert!(encode_frame(&Frame::Request {
                id: "request-1".into(),
                path: "/mcp".into(),
                method: "POST".into(),
                headers: with_access(value),
                body: String::new(),
            })
            .is_err());
        }
        // The relay never answers with it.
        assert!(encode_frame(&Frame::Response {
            id: "response-1".into(),
            status: 200,
            headers: with_access("write"),
        })
        .is_err());
        assert_invalid(
            r#"{"v":1,"type":"response","id":"id","status":200,"headers":{"x-wenlan-access":"write"}}"#,
        );
    }

    #[test]
    fn wire_shapes_have_exact_keys_and_reject_unknown_fields() {
        assert_eq!(
            keys(&encode_frame(&request("")).unwrap()),
            vec![
                "body".to_owned(),
                "headers".to_owned(),
                "id".to_owned(),
                "method".to_owned(),
                "path".to_owned(),
                "type".to_owned(),
                "v".to_owned()
            ]
        );
        assert_eq!(
            keys(&encode_frame(&response()).unwrap()),
            vec![
                "headers".to_owned(),
                "id".to_owned(),
                "status".to_owned(),
                "type".to_owned(),
                "v".to_owned()
            ]
        );
        assert_eq!(
            keys(&encode_frame(&chunk(&encode_body(&[1]).unwrap())).unwrap()),
            vec![
                "body".to_owned(),
                "id".to_owned(),
                "seq".to_owned(),
                "type".to_owned(),
                "v".to_owned()
            ]
        );
        assert_eq!(
            keys(&encode_frame(&Frame::End { id: "id".into() }).unwrap()),
            vec!["id".to_owned(), "type".to_owned(), "v".to_owned()]
        );
        assert_eq!(
            keys(&encode_frame(&Frame::Cancel { id: "id".into() }).unwrap()),
            vec!["id".to_owned(), "type".to_owned(), "v".to_owned()]
        );
        assert_eq!(
            keys(
                &encode_frame(&Frame::Credit {
                    id: "id".into(),
                    seq: 0
                })
                .unwrap()
            ),
            vec![
                "id".to_owned(),
                "seq".to_owned(),
                "type".to_owned(),
                "v".to_owned()
            ]
        );

        let valid = r#"{"v":1,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{},"body":""}"#;
        assert_eq!(
            decode_frame(valid).unwrap(),
            Frame::Request {
                id: "id".into(),
                path: "/mcp".into(),
                method: "POST".into(),
                headers: HeaderMap::new(),
                body: String::new(),
            }
        );
        assert_invalid(
            r#"{"v":1,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{},"body":"","extra":true}"#,
        );
        assert_invalid(
            r#"{"v":2,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{},"body":""}"#,
        );
        assert_invalid(
            r#"{"v":1,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{},"body":null}"#,
        );
        assert_invalid(
            r#"{"v":1,"type":"response","id":"id","status":200,"headers":{},"body":""}"#,
        );
    }

    #[test]
    fn request_paths_methods_and_response_status_are_restricted() {
        for path in [
            "/connector-info?x=1",
            "https://backend.invalid/mcp",
            "/other",
        ] {
            assert!(encode_frame(&Frame::Request {
                id: "request-1".into(),
                path: path.into(),
                method: "POST".into(),
                headers: HeaderMap::new(),
                body: String::new(),
            })
            .is_err());
        }
        assert!(encode_frame(&Frame::Request {
            id: "request-1".into(),
            path: "/connector-info".into(),
            method: "POST".into(),
            headers: HeaderMap::new(),
            body: String::new(),
        })
        .is_err());
        assert!(encode_frame(&Frame::Request {
            id: "request-1".into(),
            path: "/mcp".into(),
            method: "PUT".into(),
            headers: HeaderMap::new(),
            body: String::new(),
        })
        .is_err());

        for status in [200, 599] {
            let frame = Frame::Response {
                id: "response-1".into(),
                status,
                headers: [("content-type".into(), "application/json".into())]
                    .into_iter()
                    .collect(),
            };
            assert!(encode_frame(&frame).is_ok());
        }
        for status in [0, 199, 600] {
            let frame = Frame::Response {
                id: "response-1".into(),
                status,
                headers: [("content-type".into(), "application/json".into())]
                    .into_iter()
                    .collect(),
            };
            assert!(encode_frame(&frame).is_err());
        }
        assert_invalid(r#"{"v":1,"type":"response","id":"id","status":200.5,"headers":{}}"#);
    }

    #[test]
    fn canonical_base64_rejects_nonzero_trailing_bits() {
        for value in [
            " ", "YQ", "YQ==\n", "YQ== ", "YQ-=", "YQ_=", "!!!!", "YWI===", "YR==",
        ] {
            assert!(decode_body(value).is_err());
            assert_invalid(&format!(
                r#"{{"v":1,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{{}},"body":"{value}"}}"#
            ));
        }
    }

    #[test]
    fn sequence_accepts_safe_maximum_and_rejects_other_values() {
        for (value, expected) in [
            (
                format!(r#"{{"v":1,"type":"credit","id":"id","seq":{MAX_SAFE_SEQUENCE}}}"#),
                MAX_SAFE_SEQUENCE,
            ),
            (r#"{"v":1,"type":"credit","id":"id","seq":1.0}"#.into(), 1),
            (r#"{"v":1,"type":"credit","id":"id","seq":1e0}"#.into(), 1),
        ] {
            assert_eq!(
                decode_frame(&value).unwrap(),
                Frame::Credit {
                    id: "id".into(),
                    seq: expected,
                }
            );
        }
        for value in [
            format!(
                r#"{{"v":1,"type":"credit","id":"id","seq":{}}}"#,
                MAX_SAFE_SEQUENCE + 1
            ),
            r#"{"v":1,"type":"credit","id":"id","seq":-1}"#.into(),
            r#"{"v":1,"type":"credit","id":"id","seq":1.5}"#.into(),
        ] {
            assert_invalid(&value);
        }
    }

    #[test]
    fn wire_limit_is_checked_before_parsing_and_debug_is_redacted() {
        assert_invalid(&"{".to_string().repeat(MAX_WIRE_BYTES + 1));
        assert_invalid(&format!(
            r#"{{"v":1,"type":"request","id":"id","path":"/mcp","method":"POST","headers":{{}},"body":"{}"}}"#,
            "A".repeat(MAX_WIRE_BYTES)
        ));

        let secret = "PRIVATE_WIRE_PAYLOAD_123";
        let frame = Frame::Request {
            id: "id".into(),
            path: "/mcp".into(),
            method: "POST".into(),
            headers: [("content-type".into(), format!("{secret}\n"))]
                .into_iter()
                .collect(),
            body: encode_body(secret.as_bytes()).unwrap(),
        };
        let debug = format!("{frame:?}");
        assert!(!debug.contains(secret));
        let error = encode_frame(&frame).expect_err("header should be invalid");
        assert!(!error.to_string().contains(secret));
        assert!(!format!("{error:?}").contains(secret));

        let malformed = json!({
            "v": 1,
            "type": "request",
            "id": format!("{secret}!"),
            "path": "/mcp",
            "method": "POST",
            "headers": {},
            "body": ""
        })
        .to_string();
        let error = decode_frame(&malformed).expect_err("secret id should be invalid");
        assert_eq!(error.to_string(), INVALID_MESSAGE);
        assert!(!format!("{error:?}").contains(secret));
    }
}
