// SPDX-License-Identifier: Apache-2.0
//! ChatGPT conversation export parser.
//!
//! See `docs/superpowers/specs/2026-04-10-chatgpt-claude-import-design.md`
//! section "Format details — ChatGPT" for the schema reference.

use crate::chat_import::types::{
    ChatExportParser, ImportError, MessageRole, ParsedConversation, ParsedMessage, Vendor,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::io::{Cursor, Read};

/// Deserialize a `bool` that tolerates explicit JSON `null` (treated as `false`).
///
/// `#[serde(default)]` alone only fires on *absent* keys; an explicit `null`
/// still fails. ChatGPT exports sometimes emit `null` for optional flags.
fn bool_from_null_or_absent<'de, D>(de: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<bool>::deserialize(de)?.unwrap_or(false))
}

/// Deserialize a `Vec<serde_json::Value>` that tolerates explicit JSON `null`
/// (treated as an empty vec). Same rationale as `bool_from_null_or_absent`.
fn vec_from_null_or_absent<'de, D>(de: D) -> Result<Vec<serde_json::Value>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<serde_json::Value>>::deserialize(de)?.unwrap_or_default())
}

pub struct ChatGptParser;

impl ChatGptParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ChatGptParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatExportParser for ChatGptParser {
    fn vendor(&self) -> Vendor {
        Vendor::ChatGpt
    }

    fn can_parse(&self, archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> bool {
        let names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .collect();
        names.iter().any(|n| n == "message_feedback.json")
            && (names.iter().any(|n| n == "conversations.json")
                || names.iter().any(|n| n == "export_manifest.json"))
    }

    fn parse(
        &self,
        archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    ) -> Result<Vec<ParsedConversation>, ImportError> {
        let files = conversation_files(archive)?;
        let mut out = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        for path in files {
            let mut entry = archive
                .by_name(&path)
                .map_err(|e| ImportError::InvalidFormat {
                    reason: format!("manifest conversation file '{path}' not found: {e}"),
                })?;
            let mut buf = String::new();
            entry
                .read_to_string(&mut buf)
                .map_err(|source| ImportError::ZipEntryRead {
                    path: path.clone(),
                    source,
                })?;
            drop(entry);

            let raw: Vec<RawChatGptConversation> =
                serde_json::from_str(&buf).map_err(|source| ImportError::Json {
                    path: path.clone(),
                    source,
                })?;
            for raw_conv in raw {
                if !seen_ids.insert(raw_conv.id.clone()) {
                    return Err(ImportError::InvalidFormat {
                        reason: format!(
                            "duplicate conversation id '{}' across export files",
                            raw_conv.id
                        ),
                    });
                }
                if raw_conv.is_do_not_remember {
                    continue;
                }
                let messages = walk_current_branch(&raw_conv);
                if messages.is_empty() {
                    continue;
                }
                out.push(ParsedConversation {
                    external_id: raw_conv.id,
                    vendor: Vendor::ChatGpt,
                    title: raw_conv.title.filter(|t| !t.is_empty()),
                    created_at: float_to_utc(raw_conv.create_time),
                    summary: None,
                    messages,
                });
            }
        }
        Ok(out)
    }
}

/// Resolve the logical conversations file, using the export manifest as the
/// authority when present so a missing shard cannot silently drop history.
fn conversation_files(
    archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
) -> Result<Vec<String>, ImportError> {
    if archive
        .file_names()
        .any(|name| name == "export_manifest.json")
    {
        let mut entry =
            archive
                .by_name("export_manifest.json")
                .map_err(|e| ImportError::InvalidFormat {
                    reason: format!("export_manifest.json not found: {e}"),
                })?;
        let mut buf = String::new();
        entry
            .read_to_string(&mut buf)
            .map_err(|source| ImportError::ZipEntryRead {
                path: "export_manifest.json".into(),
                source,
            })?;
        drop(entry);
        let manifest: serde_json::Value =
            serde_json::from_str(&buf).map_err(|source| ImportError::Json {
                path: "export_manifest.json".into(),
                source,
            })?;
        let manifest = manifest
            .as_object()
            .ok_or_else(|| ImportError::InvalidFormat {
                reason: "export manifest must be a JSON object".into(),
            })?;
        let Some(logical_files_value) = manifest.get("logical_files") else {
            if let Some(export_files) = manifest.get("export_files") {
                let export_files =
                    export_files
                        .as_array()
                        .ok_or_else(|| ImportError::InvalidFormat {
                            reason: "export manifest export_files must be an array".into(),
                        })?;
                for file in export_files {
                    let file = file.as_object().ok_or_else(|| ImportError::InvalidFormat {
                        reason: "export manifest export_files entries must be objects".into(),
                    })?;
                    let path = file
                        .get("path")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| ImportError::InvalidFormat {
                            reason: "export manifest export_files entries need a string path"
                                .into(),
                        })?;
                    if is_numbered_conversation_shard(path) {
                        return Err(ImportError::InvalidFormat {
                            reason: "export manifest inventory declares sharded conversations without logical_files".into(),
                        });
                    }
                }
            }
            let has_legacy_root = archive
                .file_names()
                .any(|name| name == "conversations.json");
            let has_numbered_shards = archive.file_names().any(is_numbered_conversation_shard);
            if has_legacy_root && !has_numbered_shards {
                return Ok(vec!["conversations.json".into()]);
            }
            return Err(ImportError::InvalidFormat {
                reason: "export manifest has no conversations.json logical file".into(),
            });
        };
        let logical_files =
            logical_files_value
                .as_object()
                .ok_or_else(|| ImportError::InvalidFormat {
                    reason: "export manifest logical_files must be an object".into(),
                })?;
        let logical_value =
            logical_files
                .get("conversations.json")
                .ok_or_else(|| ImportError::InvalidFormat {
                    reason: "export manifest has no conversations.json logical file".into(),
                })?;
        let logical: RawLogicalFile =
            serde_json::from_value(logical_value.clone()).map_err(|source| ImportError::Json {
                path: "export_manifest.json logical_files.conversations.json".into(),
                source,
            })?;
        if logical.files.is_empty() {
            return Err(ImportError::InvalidFormat {
                reason: "manifest conversations.json file list is empty".into(),
            });
        }
        if logical
            .files
            .iter()
            .any(|path| !is_conversation_export_path(path))
        {
            return Err(ImportError::InvalidFormat {
                reason: "manifest contains an invalid conversations file path".into(),
            });
        }
        let mut unique = std::collections::HashSet::new();
        if logical
            .files
            .iter()
            .any(|path| !unique.insert(path.as_str()))
        {
            return Err(ImportError::InvalidFormat {
                reason: "manifest contains duplicate conversations file paths".into(),
            });
        }
        if logical.sharded {
            if logical.shard_count != Some(logical.files.len()) {
                return Err(ImportError::InvalidFormat {
                    reason: format!(
                        "manifest shard_count does not match {} conversation files",
                        logical.files.len()
                    ),
                });
            }
        } else if logical.files.len() != 1 || logical.shard_count.is_some() {
            return Err(ImportError::InvalidFormat {
                reason: "unsharded manifest must name exactly one conversation file".into(),
            });
        }
        for path in &logical.files {
            if !archive.file_names().any(|name| name == path) {
                return Err(ImportError::InvalidFormat {
                    reason: format!("manifest conversation file '{path}' is missing from archive"),
                });
            }
        }
        if archive
            .file_names()
            .filter(|name| is_conversation_export_path(name))
            .any(|name| !unique.contains(name))
        {
            return Err(ImportError::InvalidFormat {
                reason: "archive contains a conversation file omitted from the export manifest"
                    .into(),
            });
        }
        Ok(logical.files)
    } else if archive
        .file_names()
        .any(|name| name == "conversations.json")
    {
        Ok(vec!["conversations.json".into()])
    } else {
        Err(ImportError::InvalidFormat {
            reason: "conversations.json or export manifest not found".into(),
        })
    }
}

fn is_conversation_export_path(path: &str) -> bool {
    path == "conversations.json" || is_numbered_conversation_shard(path)
}

fn is_numbered_conversation_shard(path: &str) -> bool {
    let Some(shard) = path
        .strip_prefix("conversations-")
        .and_then(|p| p.strip_suffix(".json"))
    else {
        return false;
    };
    !shard.is_empty() && shard.bytes().all(|byte| byte.is_ascii_digit())
}

#[derive(Deserialize)]
struct RawLogicalFile {
    files: Vec<String>,
    #[serde(default)]
    shard_count: Option<usize>,
    #[serde(default)]
    sharded: bool,
}

/// Convert an optional Unix timestamp (as f64) to a UTC DateTime.
fn float_to_utc(opt: Option<f64>) -> Option<DateTime<Utc>> {
    let secs = opt?;
    DateTime::from_timestamp(secs as i64, (secs.fract().abs() * 1e9) as u32)
}

/// Extract user-visible text from a ChatGPT message content block.
///
/// Handles `text` and `multimodal_text` content types by collecting string
/// parts and joining them with `"\n"`. All other content types are skipped
/// (returning an empty string).
fn extract_content(content: &RawChatGptContent) -> String {
    match content.content_type.as_str() {
        "text" | "multimodal_text" => content
            .parts
            .iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<&str>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Walk the parent chain from `current_node` backward through the mapping,
/// producing messages in chronological order for the current conversation branch.
///
/// ChatGPT exports use a tree structure where `current_node` identifies the
/// leaf of the active branch. Nodes with `message: null` act as root sentinels.
fn walk_current_branch(raw: &RawChatGptConversation) -> Vec<ParsedMessage> {
    let current_node_id = match &raw.current_node {
        Some(id) => id,
        None => return Vec::new(),
    };

    if !raw.mapping.contains_key(current_node_id.as_str()) {
        return Vec::new();
    }

    // Walk backward from current_node via parent links.
    let mut chain: Vec<&RawChatGptNode> = Vec::new();
    let mut current_id: &str = current_node_id.as_str();
    let mut reached_terminal = false;

    for _ in 0..100_000 {
        let node = match raw.mapping.get(current_id) {
            Some(n) => n,
            None => {
                // Orphaned parent — stop here.
                reached_terminal = true;
                break;
            }
        };
        chain.push(node);

        match &node.parent {
            None => {
                // Reached root (no parent).
                reached_terminal = true;
                break;
            }
            Some(parent_id) => {
                // Check if the parent node exists and has a message.
                // A node with message: None is the root sentinel — stop before adding it.
                match raw.mapping.get(parent_id.as_str()) {
                    None => {
                        // Orphaned parent.
                        reached_terminal = true;
                        break;
                    }
                    Some(parent_node) if parent_node.message.is_none() => {
                        // Root sentinel node.
                        reached_terminal = true;
                        break;
                    }
                    Some(_) => {
                        current_id = parent_id.as_str();
                    }
                }
            }
        }
    }

    if !reached_terminal {
        log::warn!(
            "walk_current_branch: chain did not reach terminal after walking {} nodes; skipping malformed conversation",
            chain.len()
        );
        return Vec::new();
    }

    // Walked backward — reverse for chronological order.
    chain.reverse();

    // Convert to ParsedMessage, filtering by role and content.
    let individual: Vec<ParsedMessage> = chain
        .into_iter()
        .filter_map(|node| {
            let msg = node.message.as_ref()?;
            let role = match msg.author.role.as_str() {
                "user" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                _ => return None,
            };
            let content = extract_content(&msg.content);
            if content.is_empty() {
                return None;
            }
            Some(ParsedMessage {
                role,
                content,
                created_at: float_to_utc(msg.create_time),
            })
        })
        .collect();

    super::pair_messages(individual)
}

// ---- Raw types matching the ChatGPT export JSON schema ----

#[derive(Deserialize)]
struct RawChatGptConversation {
    id: String,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    create_time: Option<f64>,
    #[serde(default)]
    current_node: Option<String>,
    mapping: HashMap<String, RawChatGptNode>,
    #[serde(default, deserialize_with = "bool_from_null_or_absent")]
    is_do_not_remember: bool,
}

#[derive(Deserialize)]
struct RawChatGptNode {
    #[serde(default)]
    parent: Option<String>,
    #[serde(default)]
    message: Option<RawChatGptMessage>,
}

#[derive(Deserialize)]
struct RawChatGptMessage {
    author: RawChatGptAuthor,
    content: RawChatGptContent,
    #[serde(default)]
    create_time: Option<f64>,
}

#[derive(Deserialize)]
struct RawChatGptAuthor {
    role: String,
}

#[derive(Deserialize)]
struct RawChatGptContent {
    content_type: String,
    #[serde(default, deserialize_with = "vec_from_null_or_absent")]
    parts: Vec<serde_json::Value>,
    #[allow(dead_code)]
    #[serde(default)]
    text: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_import::types::{MessageRole, Vendor};
    use std::io::{Cursor, Write};

    // ---- helpers ----

    fn make_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let cursor = Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options: zip::write::FileOptions<()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            for (name, data) in entries {
                zip.start_file(*name, options).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        buf
    }

    fn parse_zip(zip_bytes: &[u8]) -> Result<Vec<ParsedConversation>, ImportError> {
        let cursor = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).expect("valid zip");
        ChatGptParser::new().parse(&mut archive)
    }

    fn can_parse_zip(zip_bytes: &[u8]) -> bool {
        let cursor = Cursor::new(zip_bytes);
        let mut archive = zip::ZipArchive::new(cursor).expect("valid zip");
        ChatGptParser::new().can_parse(&mut archive)
    }

    /// Build a minimal single-conversation ChatGPT conversations.json JSON string.
    ///
    /// `nodes` is a list of `(id, parent_id, role, content_type, content_text, create_time)`.
    /// `current_node` is the leaf node id.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::type_complexity)]
    fn build_conversations_json(
        conv_id: &str,
        title: Option<&str>,
        create_time: Option<f64>,
        is_do_not_remember: bool,
        current_node: Option<&str>,
        nodes: &[(&str, Option<&str>, &str, &str, &str, Option<f64>)],
    ) -> String {
        let title_val = match title {
            Some(t) => serde_json::json!(t),
            None => serde_json::Value::Null,
        };
        let create_time_val = match create_time {
            Some(t) => serde_json::json!(t),
            None => serde_json::Value::Null,
        };
        let current_node_val = match current_node {
            Some(n) => serde_json::json!(n),
            None => serde_json::Value::Null,
        };

        let mut mapping = serde_json::Map::new();
        for (id, parent, role, content_type, content_text, node_create_time) in nodes {
            let node_create_time_val = match node_create_time {
                Some(t) => serde_json::json!(t),
                None => serde_json::Value::Null,
            };
            let parts: serde_json::Value =
                if *content_type == "text" || *content_type == "multimodal_text" {
                    serde_json::json!([content_text])
                } else {
                    serde_json::json!([])
                };
            let content = serde_json::json!({
                "content_type": content_type,
                "parts": parts,
                "text": content_text
            });
            let message = serde_json::json!({
                "author": {"role": role},
                "content": content,
                "create_time": node_create_time_val
            });
            let parent_val = match parent {
                Some(p) => serde_json::json!(p),
                None => serde_json::Value::Null,
            };
            let node = serde_json::json!({
                "id": id,
                "parent": parent_val,
                "message": message
            });
            mapping.insert(id.to_string(), node);
        }

        let conv = serde_json::json!([{
            "id": conv_id,
            "title": title_val,
            "create_time": create_time_val,
            "current_node": current_node_val,
            "is_do_not_remember": is_do_not_remember,
            "mapping": serde_json::Value::Object(mapping)
        }]);
        conv.to_string()
    }

    /// A minimal valid ChatGPT conversations.json with one user+assistant exchange.
    fn min_chatgpt_conversations_json() -> String {
        build_conversations_json(
            "conv-gpt-1",
            Some("Test Conv"),
            Some(1_700_000_000.0),
            false,
            Some("node-asst"),
            &[
                (
                    "node-user",
                    None,
                    "user",
                    "text",
                    "hello there",
                    Some(1_700_000_001.0),
                ),
                (
                    "node-asst",
                    Some("node-user"),
                    "assistant",
                    "text",
                    "hi back",
                    Some(1_700_000_002.0),
                ),
            ],
        )
    }

    // ---- can_parse tests ----

    #[test]
    fn can_parse_accepts_archive_with_conversations_and_message_feedback() {
        let zip_bytes = make_zip(&[
            ("conversations.json", b"[]"),
            ("message_feedback.json", b"[]"),
        ]);
        assert!(
            can_parse_zip(&zip_bytes),
            "should accept zip with both conversations.json and message_feedback.json"
        );
    }

    #[test]
    fn can_parse_rejects_claude_archive() {
        // Claude uses conversations.json + users.json (no message_feedback.json)
        let zip_bytes = make_zip(&[("conversations.json", b"[]"), ("users.json", b"[]")]);
        assert!(
            !can_parse_zip(&zip_bytes),
            "should reject Claude archive that lacks message_feedback.json"
        );
    }

    #[test]
    fn can_parse_rejects_random_archive() {
        let zip_bytes = make_zip(&[("readme.txt", b"nothing relevant here")]);
        assert!(
            !can_parse_zip(&zip_bytes),
            "should reject archive with unrelated files"
        );
    }

    #[test]
    fn can_parse_requires_message_feedback_json() {
        // Only conversations.json without the ChatGPT-specific marker file
        let zip_bytes = make_zip(&[("conversations.json", b"[]")]);
        assert!(
            !can_parse_zip(&zip_bytes),
            "should reject archive missing message_feedback.json"
        );
    }

    // ---- parse tests ----

    fn manifest(files: &[&str], shard_count: usize) -> String {
        serde_json::json!({
            "version": 1,
            "logical_files": {"conversations.json": {
                "files": files,
                "shard_count": shard_count,
                "sharded": true
            }}
        })
        .to_string()
    }

    #[test]
    fn parse_manifest_shards_in_list_order() {
        let first = build_conversations_json(
            "shard-one",
            Some("First"),
            None,
            false,
            Some("node-asst"),
            &[
                ("node-user", None, "user", "text", "first user", None),
                (
                    "node-asst",
                    Some("node-user"),
                    "assistant",
                    "text",
                    "first reply",
                    None,
                ),
            ],
        );
        let second = build_conversations_json(
            "shard-two",
            Some("Second"),
            None,
            false,
            Some("node-asst"),
            &[
                ("node-user", None, "user", "text", "second user", None),
                (
                    "node-asst",
                    Some("node-user"),
                    "assistant",
                    "text",
                    "second reply",
                    None,
                ),
            ],
        );
        let zip = make_zip(&[
            ("message_feedback.json", b"[]"),
            (
                "export_manifest.json",
                manifest(&["conversations-000.json", "conversations-001.json"], 2).as_bytes(),
            ),
            ("conversations-000.json", first.as_bytes()),
            ("conversations-001.json", second.as_bytes()),
        ]);
        let batch = crate::chat_import::dispatch_parse(&zip).expect("sharded export parses");
        assert_eq!(batch.vendor, Vendor::ChatGpt);
        assert_eq!(
            batch
                .conversations
                .iter()
                .map(|c| c.external_id.as_str())
                .collect::<Vec<_>>(),
            vec!["shard-one", "shard-two"]
        );
        assert!(batch.conversations[0].messages[0]
            .content
            .contains("first user"));
        assert!(batch.conversations[1].messages[0]
            .content
            .contains("second user"));
    }

    #[test]
    fn inventory_only_manifest_preserves_legacy_root_conversations() {
        let inventory_manifest = serde_json::json!({
            "export_files": [{ "path": "conversations.json", "size_bytes": 1 }]
        })
        .to_string();
        let zip = make_zip(&[
            (
                "conversations.json",
                min_chatgpt_conversations_json().as_bytes(),
            ),
            ("message_feedback.json", b"[]"),
            ("export_manifest.json", inventory_manifest.as_bytes()),
        ]);
        assert_eq!(
            parse_zip(&zip)
                .expect("legacy export with inventory manifest parses")
                .len(),
            1
        );
    }

    #[test]
    fn inventory_only_manifest_does_not_fallback_when_a_shard_is_missing() {
        let inventory_manifest = serde_json::json!({
            "export_files": [
                { "path": "conversations.json", "size_bytes": 1 },
                { "path": "conversations-001.json", "size_bytes": 1 }
            ]
        })
        .to_string();
        let zip = make_zip(&[
            (
                "conversations.json",
                min_chatgpt_conversations_json().as_bytes(),
            ),
            ("message_feedback.json", b"[]"),
            ("export_manifest.json", inventory_manifest.as_bytes()),
        ]);
        assert!(matches!(
            parse_zip(&zip),
            Err(ImportError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn malformed_present_export_inventory_does_not_fallback_to_legacy_root() {
        for export_files in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!([null]),
            serde_json::json!([{}]),
            serde_json::json!([{ "path": null }]),
            serde_json::json!([{ "path": 7 }]),
        ] {
            let inventory_manifest =
                serde_json::json!({ "export_files": export_files }).to_string();
            let zip = make_zip(&[
                (
                    "conversations.json",
                    min_chatgpt_conversations_json().as_bytes(),
                ),
                ("message_feedback.json", b"[]"),
                ("export_manifest.json", inventory_manifest.as_bytes()),
            ]);
            assert!(matches!(
                parse_zip(&zip),
                Err(ImportError::InvalidFormat { .. })
            ));
        }
    }

    #[test]
    fn inventory_only_manifest_does_not_fallback_when_shards_exist() {
        let inventory_manifest = serde_json::json!({
            "export_files": [
                { "path": "conversations.json", "size_bytes": 1 },
                { "path": "conversations-000.json", "size_bytes": 1 }
            ]
        })
        .to_string();
        let zip = make_zip(&[
            (
                "conversations.json",
                min_chatgpt_conversations_json().as_bytes(),
            ),
            ("conversations-000.json", b"[]"),
            ("message_feedback.json", b"[]"),
            ("export_manifest.json", inventory_manifest.as_bytes()),
        ]);
        assert!(matches!(
            parse_zip(&zip),
            Err(ImportError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn malformed_present_logical_files_does_not_fallback_to_legacy_root() {
        for logical_files in [
            serde_json::Value::Null,
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({"conversations.json": null}),
            serde_json::json!({"conversations.json": []}),
            serde_json::json!("not-an-object"),
        ] {
            let inventory_manifest = serde_json::json!({
                "export_files": [{ "path": "conversations.json", "size_bytes": 1 }],
                "logical_files": logical_files,
            })
            .to_string();
            let zip = make_zip(&[
                (
                    "conversations.json",
                    min_chatgpt_conversations_json().as_bytes(),
                ),
                ("message_feedback.json", b"[]"),
                ("export_manifest.json", inventory_manifest.as_bytes()),
            ]);
            assert!(matches!(
                parse_zip(&zip),
                Err(ImportError::InvalidFormat { .. }) | Err(ImportError::Json { .. })
            ));
        }
    }

    #[test]
    fn legacy_conversations_file_still_parses() {
        let zip = make_zip(&[
            (
                "conversations.json",
                min_chatgpt_conversations_json().as_bytes(),
            ),
            ("message_feedback.json", b"[]"),
        ]);
        assert_eq!(parse_zip(&zip).expect("legacy export parses").len(), 1);
    }

    #[test]
    fn can_parse_accepts_manifest_export_with_message_feedback() {
        let zip = make_zip(&[
            ("export_manifest.json", b"{}"),
            ("message_feedback.json", b"[]"),
        ]);
        assert!(can_parse_zip(&zip));
    }

    #[test]
    fn manifest_missing_shard_fails_closed() {
        let zip = make_zip(&[
            ("message_feedback.json", b"[]"),
            (
                "export_manifest.json",
                manifest(&["conversations-000.json", "conversations-001.json"], 2).as_bytes(),
            ),
            ("conversations-000.json", b"[]"),
        ]);
        assert!(matches!(
            parse_zip(&zip),
            Err(ImportError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn malformed_later_shard_fails_the_entire_parse() {
        let zip = make_zip(&[
            ("message_feedback.json", b"[]"),
            (
                "export_manifest.json",
                manifest(&["conversations-000.json", "conversations-001.json"], 2).as_bytes(),
            ),
            (
                "conversations-000.json",
                min_chatgpt_conversations_json().as_bytes(),
            ),
            ("conversations-001.json", b"{"),
        ]);
        assert!(matches!(parse_zip(&zip), Err(ImportError::Json { .. })));
    }

    #[test]
    fn invalid_manifest_file_names_fail_closed() {
        for files in [
            vec!["../conversations.json"],
            vec!["conversations-000.json", "conversations-000.json"],
        ] {
            let zip = make_zip(&[
                ("message_feedback.json", b"[]"),
                (
                    "export_manifest.json",
                    manifest(&files, files.len()).as_bytes(),
                ),
            ]);
            assert!(matches!(
                parse_zip(&zip),
                Err(ImportError::InvalidFormat { .. })
            ));
        }
    }

    #[test]
    fn manifest_shard_count_must_match() {
        let zip = make_zip(&[
            ("message_feedback.json", b"[]"),
            (
                "export_manifest.json",
                manifest(&["conversations-000.json", "conversations-001.json"], 3).as_bytes(),
            ),
            ("conversations-000.json", b"[]"),
            ("conversations-001.json", b"[]"),
        ]);
        assert!(matches!(
            parse_zip(&zip),
            Err(ImportError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn manifest_cannot_silently_omit_an_archive_shard() {
        let zip = make_zip(&[
            ("message_feedback.json", b"[]"),
            (
                "export_manifest.json",
                manifest(&["conversations-000.json"], 1).as_bytes(),
            ),
            ("conversations-000.json", b"[]"),
            (
                "conversations-001.json",
                min_chatgpt_conversations_json().as_bytes(),
            ),
        ]);
        assert!(matches!(
            parse_zip(&zip),
            Err(ImportError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn duplicate_conversation_ids_across_shards_fail_closed() {
        let duplicate = min_chatgpt_conversations_json();
        let zip = make_zip(&[
            ("message_feedback.json", b"[]"),
            (
                "export_manifest.json",
                manifest(&["conversations-000.json", "conversations-001.json"], 2).as_bytes(),
            ),
            ("conversations-000.json", duplicate.as_bytes()),
            ("conversations-001.json", duplicate.as_bytes()),
        ]);
        assert!(matches!(
            parse_zip(&zip),
            Err(ImportError::InvalidFormat { .. })
        ));
    }

    #[test]
    fn parse_simple_linear_conversation() {
        let json = min_chatgpt_conversations_json();
        let zip_bytes = make_zip(&[
            ("conversations.json", json.as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1, "expected 1 conversation");
        let conv = &conversations[0];
        assert_eq!(conv.messages.len(), 1, "expected 1 paired message");
        assert_eq!(conv.messages[0].role, MessageRole::Assistant);
        assert!(
            conv.messages[0].content.contains("hello there"),
            "content should contain user text"
        );
        assert!(
            conv.messages[0].content.contains("hi back"),
            "content should contain assistant text"
        );
        assert!(
            conv.messages[0].created_at.is_some(),
            "paired message should carry create_time"
        );
    }

    #[test]
    fn parse_walks_mapping_tree_from_current_node() {
        // Branched tree: root-sentinel → user1 → asst1 → user2 → asst2a (stale branch)
        //                                                        → asst2b (current_node)
        // current_node = asst2b; walk should pick user2 + asst2b, not asst2a.
        let json = serde_json::json!([{
            "id": "conv-branch",
            "title": "Branch",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "asst2b",
            "is_do_not_remember": false,
            "mapping": {
                "root": {
                    "id": "root",
                    "parent": null,
                    "message": null
                },
                "user1": {
                    "id": "user1",
                    "parent": "root",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["question one"], "text": "question one"},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "asst1": {
                    "id": "asst1",
                    "parent": "user1",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["answer one"], "text": "answer one"},
                        "create_time": 1_700_000_002.0_f64
                    }
                },
                "user2": {
                    "id": "user2",
                    "parent": "asst1",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["question two"], "text": "question two"},
                        "create_time": 1_700_000_003.0_f64
                    }
                },
                "asst2a": {
                    "id": "asst2a",
                    "parent": "user2",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["stale answer"], "text": "stale answer"},
                        "create_time": 1_700_000_004.0_f64
                    }
                },
                "asst2b": {
                    "id": "asst2b",
                    "parent": "user2",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["current answer"], "text": "current answer"},
                        "create_time": 1_700_000_005.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        // Should have 2 pairs: (user1, asst1) and (user2, asst2b)
        let all_content: String = conversations[0]
            .messages
            .iter()
            .map(|m| m.content.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            all_content.contains("current answer"),
            "should include asst2b content; got: {all_content}"
        );
        assert!(
            !all_content.contains("stale answer"),
            "should NOT include asst2a content; got: {all_content}"
        );
    }

    #[test]
    fn parse_skips_null_message_root_sentinel() {
        // A node with message: null at the top of the chain should terminate
        // the walk cleanly and not cause a panic or spurious output.
        let json = serde_json::json!([{
            "id": "conv-sentinel",
            "title": "Sentinel",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-asst",
            "is_do_not_remember": false,
            "mapping": {
                "node-root": {
                    "id": "node-root",
                    "parent": null,
                    "message": null
                },
                "node-user": {
                    "id": "node-user",
                    "parent": "node-root",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["greetings"], "text": "greetings"},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["hello sentinel"], "text": "hello sentinel"},
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should not error");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].messages.len(), 1);
        assert!(conversations[0].messages[0].content.contains("greetings"));
        assert!(conversations[0].messages[0]
            .content
            .contains("hello sentinel"));
    }

    #[test]
    fn parse_skips_is_do_not_remember_conversation() {
        let json = serde_json::json!([{
            "id": "conv-dnr",
            "title": "Do Not Remember",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-asst",
            "is_do_not_remember": true,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["secret question"], "text": "secret question"},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["secret answer"], "text": "secret answer"},
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(
            conversations.len(),
            0,
            "is_do_not_remember conversation should be skipped"
        );
    }

    #[test]
    fn parse_filters_tool_and_system_roles() {
        // Sequence: user → system → assistant → tool
        // Only user + assistant should survive; system + tool are dropped.
        let json = serde_json::json!([{
            "id": "conv-roles",
            "title": "Roles",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-tool",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["user msg"], "text": "user msg"},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-system": {
                    "id": "node-system",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "system"},
                        "content": {"content_type": "text", "parts": ["system msg"], "text": "system msg"},
                        "create_time": 1_700_000_002.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-system",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["assistant msg"], "text": "assistant msg"},
                        "create_time": 1_700_000_003.0_f64
                    }
                },
                "node-tool": {
                    "id": "node-tool",
                    "parent": "node-asst",
                    "message": {
                        "author": {"role": "tool"},
                        "content": {"content_type": "text", "parts": ["tool result"], "text": "tool result"},
                        "create_time": 1_700_000_004.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        // user + assistant pair → 1 paired message
        assert_eq!(conversations[0].messages.len(), 1);
        let content = &conversations[0].messages[0].content;
        assert!(content.contains("user msg"), "user text should appear");
        assert!(
            content.contains("assistant msg"),
            "assistant text should appear"
        );
        assert!(
            !content.contains("system msg"),
            "system text should be filtered out"
        );
        assert!(
            !content.contains("tool result"),
            "tool text should be filtered out"
        );
    }

    #[test]
    fn parse_extracts_multimodal_text_strings_only() {
        // parts contains a dict (asset-pointer) and a string; only the string survives.
        let json = serde_json::json!([{
            "id": "conv-mm",
            "title": "Multimodal",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-asst",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {
                            "content_type": "multimodal_text",
                            "parts": [
                                {"asset_pointer": "file-service://some-asset-id", "size_bytes": 12345},
                                "actual user text"
                            ],
                            "text": null
                        },
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["response text"], "text": "response text"},
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].messages.len(), 1);
        let content = &conversations[0].messages[0].content;
        assert!(
            content.contains("actual user text"),
            "should extract string part from multimodal_text; got: {content}"
        );
        assert!(
            !content.contains("asset_pointer"),
            "should not include asset-pointer dict contents; got: {content}"
        );
    }

    #[test]
    fn parse_drops_multimodal_text_with_no_string_parts() {
        // parts are all dicts — no strings — so the message should be dropped.
        // Without any messages, the conversation is also skipped.
        let json = serde_json::json!([{
            "id": "conv-mm-empty",
            "title": "MM No Strings",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-user",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {
                            "content_type": "multimodal_text",
                            "parts": [
                                {"asset_pointer": "file-service://img1", "size_bytes": 999},
                                {"asset_pointer": "file-service://img2", "size_bytes": 888}
                            ],
                            "text": null
                        },
                        "create_time": 1_700_000_001.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(
            conversations.len(),
            0,
            "multimodal_text with no string parts should cause conversation to be skipped"
        );
    }

    #[test]
    fn parse_skips_non_text_content_types() {
        // Messages with each non-text content_type should all be dropped.
        // We put them all in one conversation (as separate nodes) so a single
        // assertion confirms the complete filter behavior.
        let non_text_types = [
            "code",
            "thoughts",
            "reasoning_recap",
            "tether_browsing_display",
            "tether_quote",
            "user_editable_context",
            "system_error",
            "computer_output",
        ];

        for ct in &non_text_types {
            let json = serde_json::json!([{
                "id": "conv-nontexttype",
                "title": "Non-text",
                "create_time": 1_700_000_000.0_f64,
                "current_node": "node-bad",
                "is_do_not_remember": false,
                "mapping": {
                    "node-bad": {
                        "id": "node-bad",
                        "parent": null,
                        "message": {
                            "author": {"role": "user"},
                            "content": {
                                "content_type": ct,
                                "parts": ["some content that should be ignored"],
                                "text": "some content that should be ignored"
                            },
                            "create_time": 1_700_000_001.0_f64
                        }
                    }
                }
            }]);
            let zip_bytes = make_zip(&[
                ("conversations.json", json.to_string().as_bytes()),
                ("message_feedback.json", b"[]"),
            ]);
            let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
            assert_eq!(
                conversations.len(),
                0,
                "content_type '{ct}' should cause conversation to be skipped"
            );
        }
    }

    #[test]
    fn parse_handles_missing_current_node_gracefully() {
        // current_node is null — conversation should be skipped without panic.
        let json = serde_json::json!([{
            "id": "conv-no-current",
            "title": "No current node",
            "create_time": 1_700_000_000.0_f64,
            "current_node": null,
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["hi"], "text": "hi"},
                        "create_time": 1_700_000_001.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should not error");
        assert_eq!(
            conversations.len(),
            0,
            "null current_node should result in skipped conversation"
        );
    }

    #[test]
    fn parse_handles_current_node_not_in_mapping() {
        // current_node points to a non-existent mapping key — should skip, not panic.
        let json = serde_json::json!([{
            "id": "conv-missing-node",
            "title": "Missing node",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "nonexistent-id",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["hi"], "text": "hi"},
                        "create_time": 1_700_000_001.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should not error");
        assert_eq!(
            conversations.len(),
            0,
            "current_node missing from mapping should result in skipped conversation"
        );
    }

    #[test]
    fn parse_cycle_in_parent_chain_yields_empty() {
        // A → B, B → A: a 2-node cycle. The 100k-iteration cap should catch it
        // and the conversation should be skipped.
        let json = serde_json::json!([{
            "id": "conv-cycle",
            "title": "Cycle",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-b",
            "is_do_not_remember": false,
            "mapping": {
                "node-a": {
                    "id": "node-a",
                    "parent": "node-b",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["msg a"], "text": "msg a"},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-b": {
                    "id": "node-b",
                    "parent": "node-a",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["msg b"], "text": "msg b"},
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should not error");
        assert_eq!(
            conversations.len(),
            0,
            "cyclic parent chain should be caught by iteration cap and conversation skipped"
        );
    }

    #[test]
    fn parse_extracts_top_level_conversation_fields() {
        let json = serde_json::json!([{
            "id": "conv-fields",
            "title": "My ChatGPT Conversation",
            "create_time": 1_749_815_442.0_f64,
            "current_node": "node-asst",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["q"], "text": "q"},
                        "create_time": 1_749_815_443.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["a"], "text": "a"},
                        "create_time": 1_749_815_444.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        let conv = &conversations[0];
        assert_eq!(conv.external_id, "conv-fields");
        assert_eq!(conv.title.as_deref(), Some("My ChatGPT Conversation"));
        assert!(conv.created_at.is_some(), "created_at should be populated");
        assert_eq!(conv.vendor, Vendor::ChatGpt);
    }

    #[test]
    fn parse_converts_float_create_time_to_utc() {
        // create_time 1749815442.201788 should round-trip correctly to UTC.
        // 1749815442 seconds = 2025-06-13 12:10:42 UTC (approx).
        let create_time: f64 = 1_749_815_442.201_788;
        let json = serde_json::json!([{
            "id": "conv-ts",
            "title": "Timestamp test",
            "create_time": create_time,
            "current_node": "node-asst",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["query"], "text": "query"},
                        "create_time": create_time
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["answer"], "text": "answer"},
                        "create_time": create_time
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        let msg_ts = conversations[0].messages[0]
            .created_at
            .expect("message created_at should be Some");
        // The unix timestamp integer part must match.
        assert_eq!(msg_ts.timestamp(), 1_749_815_442_i64);
        // Sub-second component: ~201788 microseconds = ~201788000 nanoseconds,
        // but float precision means we only check it's non-zero.
        assert!(
            msg_ts.timestamp_subsec_nanos() > 0,
            "sub-second nanos should be non-zero"
        );
    }

    #[test]
    fn parse_returns_vendor_chatgpt() {
        let json = min_chatgpt_conversations_json();
        let zip_bytes = make_zip(&[
            ("conversations.json", json.as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].vendor, Vendor::ChatGpt);
        // source_id should start with the ChatGPT prefix
        let source_id = Vendor::ChatGpt.build_source_id(&conversations[0].external_id);
        assert!(
            source_id.starts_with("import_chatgpt_"),
            "source_id should start with import_chatgpt_ prefix; got: {source_id}"
        );
    }

    #[test]
    fn parse_keeps_a_user_message_with_no_reply() {
        // A conversation with only a single user message (no assistant
        // response) keeps that message: it is the user's own words.
        let json = serde_json::json!([{
            "id": "conv-unpaired",
            "title": "Unpaired",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-user",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["lone user message"], "text": "lone user message"},
                        "create_time": 1_700_000_001.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].messages.len(), 1);
        assert_eq!(
            conversations[0].messages[0].content,
            "User: lone user message"
        );
        assert_eq!(conversations[0].messages[0].role, MessageRole::User);
    }

    #[test]
    fn parse_text_joins_multi_part_strings_with_newline() {
        // parts: ["first", "second"] → extracted content "first\nsecond"
        let json = serde_json::json!([{
            "id": "conv-multipart",
            "title": "Multi-part",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-asst",
            "is_do_not_remember": false,
            "mapping": {
                "node-user": {
                    "id": "node-user",
                    "parent": null,
                    "message": {
                        "author": {"role": "user"},
                        "content": {
                            "content_type": "text",
                            "parts": ["first part", "second part"],
                            "text": null
                        },
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["reply"], "text": "reply"},
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed");
        assert_eq!(conversations.len(), 1);
        let content = &conversations[0].messages[0].content;
        // The user portion must join parts with "\n"
        assert!(
            content.contains("first part\nsecond part"),
            "multi-part strings should be joined with newline; got: {content}"
        );
    }

    #[test]
    fn tolerates_explicit_null_is_do_not_remember() {
        // ChatGPT sometimes emits `"is_do_not_remember": null` rather than
        // omitting the key. `#[serde(default)]` alone fails on explicit null;
        // the custom deserializer must coerce null -> false.
        let json = serde_json::json!([{
            "id": "conv-null-idnr",
            "title": "Null flag",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-asst",
            "is_do_not_remember": serde_json::Value::Null,
            "mapping": {
                "node-root": {"id": "node-root", "parent": null, "message": null},
                "node-user": {
                    "id": "node-user",
                    "parent": "node-root",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["hi"]},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {"content_type": "text", "parts": ["hello"]},
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed on null flag");
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].external_id, "conv-null-idnr");
    }

    #[test]
    fn tolerates_explicit_null_parts() {
        // Some non-text content types (e.g. `code`) emit `"parts": null`.
        // We skip the message (content is empty) but must not fail to parse.
        let json = serde_json::json!([{
            "id": "conv-null-parts",
            "title": "Null parts",
            "create_time": 1_700_000_000.0_f64,
            "current_node": "node-asst",
            "mapping": {
                "node-root": {"id": "node-root", "parent": null, "message": null},
                "node-user": {
                    "id": "node-user",
                    "parent": "node-root",
                    "message": {
                        "author": {"role": "user"},
                        "content": {"content_type": "text", "parts": ["hi"]},
                        "create_time": 1_700_000_001.0_f64
                    }
                },
                "node-asst": {
                    "id": "node-asst",
                    "parent": "node-user",
                    "message": {
                        "author": {"role": "assistant"},
                        "content": {
                            "content_type": "code",
                            "parts": serde_json::Value::Null,
                            "text": "print('hi')"
                        },
                        "create_time": 1_700_000_002.0_f64
                    }
                }
            }
        }]);
        let zip_bytes = make_zip(&[
            ("conversations.json", json.to_string().as_bytes()),
            ("message_feedback.json", b"[]"),
        ]);
        let conversations = parse_zip(&zip_bytes).expect("parse should succeed on null parts");
        // The code-node is dropped (non-text content), leaving only the user
        // message, which pair_messages keeps on its own.
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].messages[0].content, "User: hi");
    }
}
