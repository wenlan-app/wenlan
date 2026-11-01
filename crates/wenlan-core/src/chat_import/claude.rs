// SPDX-License-Identifier: Apache-2.0
//! Claude conversation export parser.
//!
//! See `docs/superpowers/specs/2026-04-10-chatgpt-claude-import-design.md`
//! section "Format details — Claude" for the schema reference.

use crate::chat_import::types::{
    ChatExportParser, ImportError, MessageRole, ParsedArtifact, ParsedConversation, ParsedMessage,
    Vendor,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{Cursor, Read};

pub struct ClaudeParser;

impl ClaudeParser {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ClaudeParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ChatExportParser for ClaudeParser {
    fn vendor(&self) -> Vendor {
        Vendor::Claude
    }

    fn can_parse(&self, archive: &mut zip::ZipArchive<Cursor<&[u8]>>) -> bool {
        let names: Vec<String> = (0..archive.len())
            .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
            .collect();
        names.iter().any(|n| n == "users.json") && names.iter().any(|n| n == "conversations.json")
    }

    fn parse(
        &self,
        archive: &mut zip::ZipArchive<Cursor<&[u8]>>,
    ) -> Result<Vec<ParsedConversation>, ImportError> {
        let mut entry =
            archive
                .by_name("conversations.json")
                .map_err(|e| ImportError::InvalidFormat {
                    reason: format!("conversations.json not found: {e}"),
                })?;
        let mut buf = String::new();
        entry
            .read_to_string(&mut buf)
            .map_err(|source| ImportError::ZipEntryRead {
                path: "conversations.json".into(),
                source,
            })?;
        drop(entry);

        let raw: Vec<RawClaudeConversation> =
            serde_json::from_str(&buf).map_err(|source| ImportError::Json {
                path: "conversations.json".into(),
                source,
            })?;

        let mut out = Vec::with_capacity(raw.len());
        for raw_conv in raw {
            let branch = current_branch(&raw_conv.chat_messages);
            let messages = branch_messages(&branch);
            if messages.is_empty() {
                continue;
            }
            let artifacts = branch_artifacts(&branch);
            out.push(ParsedConversation {
                external_id: raw_conv.uuid,
                vendor: Vendor::Claude,
                title: if raw_conv.name.is_empty() {
                    None
                } else {
                    Some(raw_conv.name)
                },
                created_at: parse_iso8601(&raw_conv.created_at),
                summary: if raw_conv.summary.is_empty() {
                    None
                } else {
                    Some(raw_conv.summary)
                },
                messages,
                artifacts,
            });
        }
        Ok(out)
    }
}

fn parse_iso8601(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

/// Root sentinel used by Claude to mark the first message of a conversation.
const ROOT_PARENT_SENTINEL: &str = "00000000-0000-4000-8000-000000000000";

/// Extract user-visible text from a Claude message.
///
/// Prefers `content[]` blocks of type `"text"`, concatenating them in order
/// (no separator — they're fragments of one logical response). Skips
/// `thinking`, `tool_use`, `tool_result` block types. Falls back to the
/// top-level `text` field only when `content[]` has no usable text blocks.
fn extract_content(raw: &RawClaudeMessage) -> String {
    let content_text: String = raw
        .content
        .iter()
        .filter(|b| b.block_type == "text")
        .map(|b| b.text.as_str())
        .collect();
    if !content_text.is_empty() {
        content_text
    } else if !raw.text.is_empty() {
        raw.text.clone()
    } else {
        String::new()
    }
}

/// Walk the parent chain from the latest message backward, producing the raw
/// messages of the "current" conversation branch in chronological order.
///
/// Claude `chat_messages` arrays are sorted by `created_at` ascending, and
/// conversations can branch when responses are regenerated. The current
/// branch is defined as the path from root to the last-created message.
fn current_branch(messages: &[RawClaudeMessage]) -> Vec<&RawClaudeMessage> {
    let Some(last) = messages.last() else {
        return Vec::new();
    };

    // Build uuid → &message lookup.
    let by_uuid: HashMap<&str, &RawClaudeMessage> =
        messages.iter().map(|m| (m.uuid.as_str(), m)).collect();

    // Walk backward from `last` via parent_message_uuid until we hit the
    // root sentinel. Cap iterations defensively.
    let mut chain: Vec<&RawClaudeMessage> = Vec::new();
    let mut current: &RawClaudeMessage = last;
    let mut reached_terminal = false;
    for _ in 0..100_000 {
        chain.push(current);
        if current.parent_message_uuid == ROOT_PARENT_SENTINEL {
            reached_terminal = true;
            break;
        }
        match by_uuid.get(current.parent_message_uuid.as_str()) {
            Some(parent) => current = parent,
            None => {
                reached_terminal = true;
                break; // Orphaned parent — stop here.
            }
        }
    }

    // If the iteration cap was exhausted without reaching a terminal
    // condition (root sentinel or orphan), the chain is malformed (likely
    // cyclic). Return empty to skip it.
    if !reached_terminal {
        log::warn!(
            "current_branch: chain did not reach root sentinel after walking {} messages; skipping malformed conversation",
            chain.len()
        );
        return Vec::new();
    }

    // Walked backward — reverse for chronological order.
    chain.reverse();
    chain
}

/// The branch's chat text as memory pairs.
///
/// Uses content[] blocks when available, falls back to top-level text, and
/// leaves out messages with no extractable text.
fn branch_messages(chain: &[&RawClaudeMessage]) -> Vec<ParsedMessage> {
    let individual: Vec<ParsedMessage> = chain
        .iter()
        .filter_map(|raw| {
            let role = match raw.sender.as_str() {
                "human" => MessageRole::User,
                "assistant" => MessageRole::Assistant,
                _ => return None,
            };
            let content = extract_content(raw);
            if content.is_empty() {
                return None;
            }
            Some(ParsedMessage {
                role,
                content,
                created_at: parse_iso8601(&raw.created_at),
            })
        })
        .collect();
    super::pair_messages(individual)
}

/// The artifacts the assistant made on this branch, each in its final state.
///
/// Claude writes an artifact with an `artifacts` tool call: `create` and
/// `rewrite` carry the whole content, `update` replaces one `old_str` with
/// `new_str`. They are replayed in order, so a later call wins. An update
/// whose `old_str` is not in the content is skipped (the export does not
/// say what Claude showed then); the artifact keeps its last whole state.
fn branch_artifacts(chain: &[&RawClaudeMessage]) -> Vec<ParsedArtifact> {
    let mut order: Vec<String> = Vec::new();
    let mut by_id: HashMap<String, ParsedArtifact> = HashMap::new();
    for raw in chain.iter().filter(|m| m.sender == "assistant") {
        let at = parse_iso8601(&raw.created_at);
        for block in raw
            .content
            .iter()
            .filter(|b| b.block_type == "tool_use" && b.name.as_deref() == Some(ARTIFACTS_TOOL))
        {
            let Some(input) = block.input.as_ref() else {
                continue;
            };
            let field = |name: &str| input.get(name).and_then(|v| v.as_str());
            let Some(id) = field("id").filter(|id| !id.trim().is_empty()) else {
                continue;
            };
            match field("command").unwrap_or("create") {
                "create" | "rewrite" => {
                    let Some(content) = field("content") else {
                        continue;
                    };
                    let previous = by_id.get(id);
                    let artifact = ParsedArtifact {
                        id: id.to_string(),
                        title: field("title")
                            .map(str::to_string)
                            .or_else(|| previous.and_then(|p| p.title.clone())),
                        kind: field("type")
                            .map(str::to_string)
                            .or_else(|| previous.map(|p| p.kind.clone()))
                            .unwrap_or_else(|| "text/plain".to_string()),
                        language: field("language")
                            .map(str::to_string)
                            .or_else(|| previous.and_then(|p| p.language.clone())),
                        content: content.to_string(),
                        created_at: at,
                    };
                    if !by_id.contains_key(id) {
                        order.push(id.to_string());
                    }
                    by_id.insert(id.to_string(), artifact);
                }
                "update" => {
                    let (Some(old), Some(new)) = (field("old_str"), field("new_str")) else {
                        continue;
                    };
                    if let Some(artifact) = by_id.get_mut(id) {
                        if !old.is_empty() && artifact.content.contains(old) {
                            artifact.content = artifact.content.replacen(old, new, 1);
                            artifact.created_at = at.or(artifact.created_at);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    order
        .into_iter()
        .filter_map(|id| by_id.remove(&id))
        .filter(|a| !a.content.trim().is_empty())
        .collect()
}

/// The tool name Claude writes artifacts with.
const ARTIFACTS_TOOL: &str = "artifacts";

// ---- Raw types matching the Claude export JSON schema ----

#[derive(Deserialize)]
struct RawClaudeConversation {
    uuid: String,
    name: String,
    #[serde(default)]
    summary: String,
    created_at: String,
    #[allow(dead_code)]
    updated_at: String,
    #[allow(dead_code)]
    account: RawClaudeAccount,
    chat_messages: Vec<RawClaudeMessage>,
}

#[derive(Deserialize)]
struct RawClaudeAccount {
    #[allow(dead_code)]
    uuid: String,
}

#[derive(Deserialize, Clone)]
struct RawClaudeMessage {
    uuid: String,
    text: String,
    #[serde(default)]
    content: Vec<RawClaudeContentBlock>,
    sender: String,
    created_at: String,
    #[allow(dead_code)]
    #[serde(default)]
    updated_at: String,
    #[allow(dead_code)]
    #[serde(default)]
    attachments: Vec<serde_json::Value>,
    #[allow(dead_code)]
    #[serde(default)]
    files: Vec<serde_json::Value>,
    parent_message_uuid: String,
}

#[derive(Deserialize, Clone)]
struct RawClaudeContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    text: String,
    /// A `tool_use` block's tool (`artifacts` for an artifact).
    #[serde(default)]
    name: Option<String>,
    /// A `tool_use` block's input.
    #[serde(default)]
    input: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat_import::types::Vendor;
    use std::io::{Cursor, Write};

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

    /// A minimal Claude conversations.json with one conversation and
    /// one user-assistant message pair.
    const MIN_CLAUDE_CONVERSATIONS_JSON: &str = r#"[
      {
        "uuid": "conv-1",
        "name": "My conversation",
        "summary": "A short summary",
        "created_at": "2026-04-01T10:00:00.000Z",
        "updated_at": "2026-04-01T10:05:00.000Z",
        "account": {"uuid": "acc-1"},
        "chat_messages": [
          {
            "uuid": "msg-1",
            "text": "hi",
            "content": [{"type": "text", "text": "hi"}],
            "sender": "human",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:00.000Z",
            "attachments": [],
            "files": [],
            "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
          },
          {
            "uuid": "msg-2",
            "text": "hello",
            "content": [{"type": "text", "text": "hello"}],
            "sender": "assistant",
            "created_at": "2026-04-01T10:00:05.000Z",
            "updated_at": "2026-04-01T10:00:05.000Z",
            "attachments": [],
            "files": [],
            "parent_message_uuid": "msg-1"
          }
        ]
      }
    ]"#;

    #[test]
    fn can_parse_recognizes_claude_export_by_conversations_json() {
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            (
                "conversations.json",
                MIN_CLAUDE_CONVERSATIONS_JSON.as_bytes(),
            ),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        assert!(parser.can_parse(&mut archive));
    }

    #[test]
    fn can_parse_rejects_non_claude_zip() {
        let zip_bytes = make_zip(&[("other.txt", b"hello")]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        assert!(!parser.can_parse(&mut archive));
    }

    #[test]
    fn can_parse_requires_users_json_alongside_conversations_json() {
        let zip_bytes = make_zip(&[("conversations.json", b"[]")]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        assert!(!parser.can_parse(&mut archive));
    }

    #[test]
    fn parse_extracts_top_level_conversation_fields() {
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            (
                "conversations.json",
                MIN_CLAUDE_CONVERSATIONS_JSON.as_bytes(),
            ),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        let conversations = parser.parse(&mut archive).expect("parse succeeds");
        assert_eq!(conversations.len(), 1);
        let conv = &conversations[0];
        assert_eq!(conv.external_id, "conv-1");
        assert_eq!(conv.vendor, Vendor::Claude);
        assert_eq!(conv.title.as_deref(), Some("My conversation"));
        assert_eq!(conv.summary.as_deref(), Some("A short summary"));
        assert!(conv.created_at.is_some());
    }

    /// Conversation with regenerated response: message msg-1 has two assistant
    /// children (msg-2a and msg-2b). The "current" branch is the one containing
    /// the latest-created message.
    const BRANCHING_CLAUDE_JSON: &str = r#"[
      {
        "uuid": "conv-branch",
        "name": "Branch test",
        "summary": "",
        "created_at": "2026-04-01T10:00:00.000Z",
        "updated_at": "2026-04-01T11:00:00.000Z",
        "account": {"uuid": "acc-1"},
        "chat_messages": [
          {
            "uuid": "msg-1",
            "text": "question",
            "content": [{"type": "text", "text": "question"}],
            "sender": "human",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:00.000Z",
            "attachments": [],
            "files": [],
            "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
          },
          {
            "uuid": "msg-2a",
            "text": "old answer",
            "content": [{"type": "text", "text": "old answer"}],
            "sender": "assistant",
            "created_at": "2026-04-01T10:00:30.000Z",
            "updated_at": "2026-04-01T10:00:30.000Z",
            "attachments": [],
            "files": [],
            "parent_message_uuid": "msg-1"
          },
          {
            "uuid": "msg-2b",
            "text": "new answer",
            "content": [{"type": "text", "text": "new answer"}],
            "sender": "assistant",
            "created_at": "2026-04-01T10:05:00.000Z",
            "updated_at": "2026-04-01T10:05:00.000Z",
            "attachments": [],
            "files": [],
            "parent_message_uuid": "msg-1"
          }
        ]
      }
    ]"#;

    #[test]
    fn parse_takes_latest_branch_when_regenerated() {
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", BRANCHING_CLAUDE_JSON.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        let conversations = parser.parse(&mut archive).expect("parse succeeds");
        assert_eq!(conversations.len(), 1);
        let conv = &conversations[0];
        // The current branch should contain the USER + *latest* assistant
        // response (msg-2b) combined into one paired message.
        assert_eq!(conv.messages.len(), 1);
        assert_eq!(conv.messages[0].role, MessageRole::Assistant);
        assert_eq!(
            conv.messages[0].content,
            "User: question\nAssistant: new answer"
        );
    }

    #[test]
    fn parse_handles_linear_chain_order() {
        // Longer linear chain to ensure parent-walk reconstructs correctly.
        let json = r#"[
          {
            "uuid": "conv-linear",
            "name": "Linear",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:30.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {"uuid": "m1", "text": "q1", "content": [{"type": "text", "text": "q1"}], "sender": "human", "created_at": "2026-04-01T10:00:00.000Z", "updated_at": "2026-04-01T10:00:00.000Z", "attachments": [], "files": [], "parent_message_uuid": "00000000-0000-4000-8000-000000000000"},
              {"uuid": "m2", "text": "a1", "content": [{"type": "text", "text": "a1"}], "sender": "assistant", "created_at": "2026-04-01T10:00:05.000Z", "updated_at": "2026-04-01T10:00:05.000Z", "attachments": [], "files": [], "parent_message_uuid": "m1"},
              {"uuid": "m3", "text": "q2", "content": [{"type": "text", "text": "q2"}], "sender": "human", "created_at": "2026-04-01T10:00:10.000Z", "updated_at": "2026-04-01T10:00:10.000Z", "attachments": [], "files": [], "parent_message_uuid": "m2"},
              {"uuid": "m4", "text": "a2", "content": [{"type": "text", "text": "a2"}], "sender": "assistant", "created_at": "2026-04-01T10:00:15.000Z", "updated_at": "2026-04-01T10:00:15.000Z", "attachments": [], "files": [], "parent_message_uuid": "m3"}
            ]
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        let conv = &parser.parse(&mut archive).unwrap()[0];
        assert_eq!(conv.messages.len(), 2);
        let contents: Vec<&str> = conv.messages.iter().map(|m| m.content.as_str()).collect();
        assert_eq!(
            contents,
            vec!["User: q1\nAssistant: a1", "User: q2\nAssistant: a2"]
        );
    }

    #[test]
    fn parse_uses_content_blocks_over_top_level_text() {
        // Top-level text is empty; actual content is in content[].
        let json = r#"[
          {
            "uuid": "conv-c",
            "name": "C",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:05.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {
                "uuid": "m1",
                "text": "",
                "content": [{"type": "text", "text": "from content block"}],
                "sender": "human",
                "created_at": "2026-04-01T10:00:00.000Z",
                "updated_at": "2026-04-01T10:00:00.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
              },
              {
                "uuid": "m2",
                "text": "",
                "content": [
                  {"type": "thinking", "text": "...internal reasoning..."},
                  {"type": "text", "text": "assistant says hi"},
                  {"type": "tool_use", "text": ""},
                  {"type": "text", "text": " and continues"}
                ],
                "sender": "assistant",
                "created_at": "2026-04-01T10:00:05.000Z",
                "updated_at": "2026-04-01T10:00:05.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "m1"
              }
            ]
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let conv = &ClaudeParser::new().parse(&mut archive).unwrap()[0];
        assert_eq!(conv.messages.len(), 1);
        // Thinking and tool_use blocks must be skipped; text blocks concatenated.
        // User + assistant are combined into a single paired message.
        assert_eq!(
            conv.messages[0].content,
            "User: from content block\nAssistant: assistant says hi and continues"
        );
    }

    #[test]
    fn parse_falls_back_to_text_when_content_has_no_text_blocks() {
        // Single user message with text fallback, plus an assistant reply so
        // we can verify fallback behavior in a paired context.
        let json = r#"[
          {
            "uuid": "conv-f",
            "name": "Fallback",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:05.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {
                "uuid": "m1",
                "text": "plain text fallback",
                "content": [{"type": "thinking", "text": "only thinking"}],
                "sender": "human",
                "created_at": "2026-04-01T10:00:00.000Z",
                "updated_at": "2026-04-01T10:00:00.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
              },
              {
                "uuid": "m2",
                "text": "reply",
                "content": [{"type": "text", "text": "reply"}],
                "sender": "assistant",
                "created_at": "2026-04-01T10:00:05.000Z",
                "updated_at": "2026-04-01T10:00:05.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "m1"
              }
            ]
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let conv = &ClaudeParser::new().parse(&mut archive).unwrap()[0];
        // The user message falls back to top-level text; paired with assistant.
        assert_eq!(conv.messages.len(), 1);
        assert_eq!(
            conv.messages[0].content,
            "User: plain text fallback\nAssistant: reply"
        );
    }

    #[test]
    fn parse_skips_message_when_both_text_and_content_empty() {
        let json = r#"[
          {
            "uuid": "conv-s",
            "name": "Skip",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:05.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {
                "uuid": "m1",
                "text": "hi",
                "content": [{"type": "text", "text": "hi"}],
                "sender": "human",
                "created_at": "2026-04-01T10:00:00.000Z",
                "updated_at": "2026-04-01T10:00:00.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
              },
              {
                "uuid": "m2",
                "text": "",
                "content": [{"type": "tool_use", "text": ""}],
                "sender": "assistant",
                "created_at": "2026-04-01T10:00:05.000Z",
                "updated_at": "2026-04-01T10:00:05.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "m1"
              }
            ]
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let conversations = ClaudeParser::new().parse(&mut archive).unwrap();
        // Empty-content assistant message is filtered out, leaving a lone
        // user message. Pairing keeps it on its own: it is the user's words.
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].messages.len(), 1);
        assert_eq!(conversations[0].messages[0].role, MessageRole::User);
    }

    #[test]
    fn parse_combines_user_assistant_pairs() {
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            (
                "conversations.json",
                MIN_CLAUDE_CONVERSATIONS_JSON.as_bytes(),
            ),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let conv = &ClaudeParser::new().parse(&mut archive).unwrap()[0];
        // MIN_CLAUDE_CONVERSATIONS_JSON has one user + one assistant message.
        // After pairing, it should be ONE message with content combining both.
        assert_eq!(conv.messages.len(), 1);
        assert_eq!(conv.messages[0].content, "User: hi\nAssistant: hello");
        assert_eq!(conv.messages[0].role, MessageRole::Assistant);
    }

    #[test]
    fn parse_keeps_unpaired_user_message() {
        let json = r#"[
          {
            "uuid": "conv-u",
            "name": "U",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:00.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {
                "uuid": "m1",
                "text": "hi",
                "content": [{"type": "text", "text": "hi"}],
                "sender": "human",
                "created_at": "2026-04-01T10:00:00.000Z",
                "updated_at": "2026-04-01T10:00:00.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "00000000-0000-4000-8000-000000000000"
              }
            ]
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let conversations = ClaudeParser::new().parse(&mut archive).unwrap();
        // A user message nobody replied to is still the user's own words.
        assert_eq!(conversations.len(), 1);
        assert_eq!(conversations[0].messages.len(), 1);
        assert_eq!(conversations[0].messages[0].content, "User: hi");
        assert_eq!(conversations[0].messages[0].role, MessageRole::User);
    }

    #[test]
    fn parse_skips_conversation_with_no_root_sentinel() {
        // Messages form a cycle: A → B → A. The parent-chain walker will
        // loop until the 100_000 iteration cap without ever reaching the
        // root sentinel. The parser should detect this and return 0
        // conversations rather than a garbage chain.
        let json = r#"[
          {
            "uuid": "conv-cycle",
            "name": "Cycle",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:05.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": [
              {
                "uuid": "msg-a",
                "text": "hi",
                "content": [{"type": "text", "text": "hi"}],
                "sender": "human",
                "created_at": "2026-04-01T10:00:00.000Z",
                "updated_at": "2026-04-01T10:00:00.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "msg-b"
              },
              {
                "uuid": "msg-b",
                "text": "hello",
                "content": [{"type": "text", "text": "hello"}],
                "sender": "assistant",
                "created_at": "2026-04-01T10:00:05.000Z",
                "updated_at": "2026-04-01T10:00:05.000Z",
                "attachments": [],
                "files": [],
                "parent_message_uuid": "msg-a"
              }
            ]
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let conversations = ClaudeParser::new().parse(&mut archive).unwrap();
        // Cyclic chain never reaches root sentinel — conversation must be skipped.
        assert_eq!(conversations.len(), 0);
    }

    #[test]
    fn parse_skips_conversations_with_zero_messages() {
        let json = r#"[
          {
            "uuid": "conv-empty",
            "name": "Empty",
            "summary": "",
            "created_at": "2026-04-01T10:00:00.000Z",
            "updated_at": "2026-04-01T10:00:00.000Z",
            "account": {"uuid": "acc-1"},
            "chat_messages": []
          }
        ]"#;
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        let parser = ClaudeParser::new();
        let conversations = parser.parse(&mut archive).expect("parse succeeds");
        assert_eq!(conversations.len(), 0);
    }

    fn parse_one(json: &str) -> Vec<ParsedConversation> {
        let zip_bytes = make_zip(&[
            ("users.json", b"[]"),
            ("conversations.json", json.as_bytes()),
        ]);
        let cursor = Cursor::new(zip_bytes.as_slice());
        let mut archive = zip::ZipArchive::new(cursor).unwrap();
        ClaudeParser::new()
            .parse(&mut archive)
            .expect("parse succeeds")
    }

    /// A conversation where Claude makes an HTML artifact, updates it, and a
    /// regenerated (stale) reply made a different one.
    const ARTIFACT_CONVERSATION_JSON: &str = r#"[
      {
        "uuid": "conv-art",
        "name": "Landing page",
        "summary": "",
        "created_at": "2026-04-01T10:00:00.000Z",
        "updated_at": "2026-04-01T10:05:00.000Z",
        "account": {"uuid": "acc-1"},
        "chat_messages": [
          {"uuid": "m1", "text": "make a page", "content": [{"type": "text", "text": "make a page"}],
           "sender": "human", "created_at": "2026-04-01T10:00:00.000Z",
           "parent_message_uuid": "00000000-0000-4000-8000-000000000000"},
          {"uuid": "m2-stale", "text": "", "sender": "assistant", "created_at": "2026-04-01T10:00:03.000Z",
           "content": [{"type": "tool_use", "name": "artifacts",
             "input": {"id": "stale", "type": "text/html", "title": "Stale", "command": "create", "content": "<p>old</p>"}}],
           "parent_message_uuid": "m1"},
          {"uuid": "m2", "text": "", "sender": "assistant", "created_at": "2026-04-01T10:00:05.000Z",
           "content": [
             {"type": "text", "text": "Here is the page."},
             {"type": "tool_use", "name": "artifacts",
              "input": {"id": "landing", "type": "text/html", "title": "Landing", "command": "create",
                        "content": "<h1>Hello</h1>"}},
             {"type": "tool_result", "name": "artifacts", "content": [{"type": "text", "text": "OK"}]}
           ],
           "parent_message_uuid": "m1"},
          {"uuid": "m3", "text": "say hi", "content": [{"type": "text", "text": "say hi instead"}],
           "sender": "human", "created_at": "2026-04-01T10:01:00.000Z", "parent_message_uuid": "m2"},
          {"uuid": "m4", "text": "", "sender": "assistant", "created_at": "2026-04-01T10:01:05.000Z",
           "content": [
             {"type": "tool_use", "name": "artifacts",
              "input": {"id": "landing", "command": "update", "old_str": "Hello", "new_str": "Hi"}},
             {"type": "tool_use", "name": "artifacts",
              "input": {"id": "landing", "command": "update", "old_str": "not there", "new_str": "x"}},
             {"type": "tool_use", "name": "artifacts",
              "input": {"id": "notes", "type": "application/vnd.ant.code", "language": "python",
                        "title": "Script", "command": "create", "content": "print('hi')"}},
             {"type": "text", "text": "Done."}
           ],
           "parent_message_uuid": "m3"}
        ]
      }
    ]"#;

    #[test]
    fn artifacts_on_the_current_branch_are_kept_in_their_final_state() {
        let conversations = parse_one(ARTIFACT_CONVERSATION_JSON);
        assert_eq!(conversations.len(), 1);
        let conv = &conversations[0];
        // The chat text is unchanged by the artifact blocks.
        assert_eq!(conv.messages.len(), 2);
        assert_eq!(
            conv.messages[0].content,
            "User: make a page\nAssistant: Here is the page."
        );
        let ids: Vec<&str> = conv.artifacts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["landing", "notes"],
            "the stale branch's artifact is not kept"
        );
        let landing = &conv.artifacts[0];
        assert_eq!(
            landing.content, "<h1>Hi</h1>",
            "the update applied, the unmatched one skipped"
        );
        assert_eq!(landing.kind, "text/html");
        assert_eq!(landing.title.as_deref(), Some("Landing"));
        assert_eq!(landing.extension(), "html");
        assert_eq!(
            landing.created_at,
            parse_iso8601("2026-04-01T10:01:05.000Z"),
            "the time of the last write"
        );
        let notes = &conv.artifacts[1];
        assert_eq!(notes.language.as_deref(), Some("python"));
        assert_eq!(notes.extension(), "py");
    }

    #[test]
    fn a_conversation_without_artifact_calls_keeps_none() {
        let conversations = parse_one(MIN_CLAUDE_CONVERSATIONS_JSON);
        assert!(conversations[0].artifacts.is_empty());
    }
}
