// SPDX-License-Identifier: Apache-2.0
//! Shared types for the chat conversation importer.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Vendor of a chat conversation export.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Vendor {
    ChatGpt,
    Claude,
}

impl Vendor {
    pub fn source_id_prefix(&self) -> &'static str {
        match self {
            Vendor::ChatGpt => "import_chatgpt_",
            Vendor::Claude => "import_claude_",
        }
    }

    pub fn build_source_id(&self, external_id: &str) -> String {
        format!("{}{}", self.source_id_prefix(), external_id)
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Vendor::ChatGpt => "chatgpt",
            Vendor::Claude => "claude",
        }
    }

    /// The vendor's name as the person knows it.
    pub fn display_name(&self) -> &'static str {
        match self {
            Vendor::ChatGpt => "ChatGPT",
            Vendor::Claude => "Claude",
        }
    }

    /// Where the conversation opens in the vendor's own app, so "open in the
    /// original AI tool" works. Both vendors key the web URL by the export's
    /// conversation id.
    pub fn conversation_url(&self, external_id: &str) -> String {
        match self {
            Vendor::ChatGpt => format!("https://chatgpt.com/c/{external_id}"),
            Vendor::Claude => format!("https://claude.ai/chat/{external_id}"),
        }
    }

    /// The vendor and conversation id behind an import `source_id`
    /// (`import_claude_{id}` / `import_chatgpt_{id}`).
    pub fn from_source_id(source_id: &str) -> Option<(Self, &str)> {
        [Vendor::Claude, Vendor::ChatGpt]
            .into_iter()
            .find_map(|vendor| {
                source_id
                    .strip_prefix(vendor.source_id_prefix())
                    .filter(|rest| !rest.is_empty())
                    .map(|rest| (vendor, rest))
            })
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "chatgpt" => Some(Vendor::ChatGpt),
            "claude" => Some(Vendor::Claude),
            _ => None,
        }
    }
}

/// Role of a message in a conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    System,
    Tool,
}

/// One message in a parsed conversation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedMessage {
    pub role: MessageRole,
    pub content: String,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// One conversation parsed from a vendor export.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedConversation {
    pub external_id: String,
    pub vendor: Vendor,
    pub title: Option<String>,
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
    pub summary: Option<String>,
    pub messages: Vec<ParsedMessage>,
    /// Things the AI made in the conversation that are not chat text: Claude
    /// artifacts and ChatGPT canvases, each in its final state on the
    /// conversation's current branch.
    #[serde(default)]
    pub artifacts: Vec<ParsedArtifact>,
}

/// One artifact (Claude) or canvas (ChatGPT) from a conversation, kept as the
/// export holds it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ParsedArtifact {
    /// The vendor's id for it within the conversation.
    pub id: String,
    pub title: Option<String>,
    /// The export's own type: a MIME-like type for Claude (`text/html`,
    /// `image/svg+xml`, `application/vnd.ant.react`, `text/markdown`, ...),
    /// the canvas type for ChatGPT (`document`, `code/html`, `code/python`, ...).
    pub kind: String,
    /// The code language, when the export names one.
    pub language: Option<String>,
    pub content: String,
    /// When the AI last wrote it.
    pub created_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl ParsedArtifact {
    /// File extension for the kept file.
    pub fn extension(&self) -> &'static str {
        let kind = self.kind.to_ascii_lowercase();
        let lang = self.language.as_deref().unwrap_or("").to_ascii_lowercase();
        match (kind.as_str(), lang.as_str()) {
            ("text/html" | "code/html", _) | (_, "html") => "html",
            ("image/svg+xml", _) | (_, "svg") => "svg",
            ("application/vnd.ant.react" | "code/react", _) | (_, "jsx" | "tsx" | "react") => "jsx",
            ("application/vnd.ant.mermaid", _) | (_, "mermaid") => "mmd",
            ("text/markdown" | "document", _) => "md",
            (_, "python") => "py",
            (_, "javascript" | "js") => "js",
            (_, "typescript" | "ts") => "ts",
            (_, "rust") => "rs",
            (_, "css") => "css",
            (_, "json") => "json",
            (_, "sql") => "sql",
            (_, "bash" | "shell" | "sh") => "sh",
            _ if kind.starts_with("code/") => "txt",
            _ => "txt",
        }
    }

    /// Whether the content is Markdown prose the page can show as-is.
    pub fn is_markdown(&self) -> bool {
        self.extension() == "md"
    }

    /// The fence language for showing it on its page.
    pub fn fence_language(&self) -> &'static str {
        match self.extension() {
            "html" => "html",
            "svg" => "svg",
            "jsx" => "jsx",
            "mmd" => "mermaid",
            "py" => "python",
            "js" => "javascript",
            "ts" => "typescript",
            "rs" => "rust",
            "css" => "css",
            "json" => "json",
            "sql" => "sql",
            "sh" => "bash",
            _ => "text",
        }
    }
}

/// Errors emitted by the chat import subsystem.
#[derive(Debug, Error)]
pub enum ImportError {
    #[error("Failed to read zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),

    #[error("Failed to read file '{path}' from archive: {source}")]
    ZipEntryRead {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("Invalid export format: {reason}")]
    InvalidFormat { reason: String },

    #[error("JSON parse error in '{path}': {source}")]
    Json {
        path: String,
        #[source]
        source: serde_json::Error,
    },

    #[error("No parser could handle the provided archive")]
    NoParserMatched,

    #[error("Database error: {0}")]
    Db(#[from] crate::error::WenlanError),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// A parser for a specific vendor's conversation export format.
pub trait ChatExportParser: Send + Sync {
    fn vendor(&self) -> Vendor;
    fn can_parse(&self, archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>) -> bool;
    fn parse(
        &self,
        archive: &mut zip::ZipArchive<std::io::Cursor<&[u8]>>,
    ) -> Result<Vec<ParsedConversation>, ImportError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vendor_source_id_prefix_chatgpt() {
        assert_eq!(Vendor::ChatGpt.source_id_prefix(), "import_chatgpt_");
    }

    #[test]
    fn vendor_source_id_prefix_claude() {
        assert_eq!(Vendor::Claude.source_id_prefix(), "import_claude_");
    }

    #[test]
    fn vendor_build_source_id() {
        let id = Vendor::Claude.build_source_id("abc-123");
        assert_eq!(id, "import_claude_abc-123");
    }

    #[test]
    fn message_role_user_maps_both_vendors() {
        let r: MessageRole = MessageRole::User;
        assert!(matches!(r, MessageRole::User));
    }

    #[test]
    fn parsed_conversation_serializable() {
        let conv = ParsedConversation {
            external_id: "conv-1".into(),
            vendor: Vendor::Claude,
            title: Some("Hello".into()),
            created_at: None,
            summary: None,
            messages: vec![],
            artifacts: vec![],
        };
        let json = serde_json::to_string(&conv).unwrap();
        let back: ParsedConversation = serde_json::from_str(&json).unwrap();
        assert_eq!(back.external_id, "conv-1");
        assert_eq!(back.vendor, Vendor::Claude);
    }

    #[test]
    fn conversation_urls_and_source_ids_round_trip() {
        assert_eq!(
            Vendor::Claude.conversation_url("abc"),
            "https://claude.ai/chat/abc"
        );
        assert_eq!(
            Vendor::ChatGpt.conversation_url("abc"),
            "https://chatgpt.com/c/abc"
        );
        assert_eq!(
            Vendor::from_source_id("import_chatgpt_abc"),
            Some((Vendor::ChatGpt, "abc"))
        );
        assert_eq!(Vendor::from_source_id("import_claude_"), None);
        assert_eq!(Vendor::from_source_id("mem_1"), None);
    }

    #[test]
    fn artifact_files_take_the_extension_of_their_kind() {
        let a = |kind: &str, language: Option<&str>| ParsedArtifact {
            id: "a".into(),
            title: None,
            kind: kind.into(),
            language: language.map(str::to_string),
            content: String::new(),
            created_at: None,
        };
        assert_eq!(a("text/html", None).extension(), "html");
        assert_eq!(a("code/html", None).extension(), "html");
        assert_eq!(a("image/svg+xml", None).extension(), "svg");
        assert_eq!(a("application/vnd.ant.react", None).extension(), "jsx");
        assert_eq!(a("document", None).extension(), "md");
        assert!(a("text/markdown", None).is_markdown());
        assert_eq!(
            a("application/vnd.ant.code", Some("python")).extension(),
            "py"
        );
        assert_eq!(a("code/python", Some("python")).extension(), "py");
        assert_eq!(a("something/else", None).extension(), "txt");
    }

    #[test]
    fn a_conversation_without_artifacts_still_deserializes() {
        let back: ParsedConversation = serde_json::from_str(
            r#"{"external_id":"c","vendor":"claude","title":null,"created_at":null,"summary":null,"messages":[]}"#,
        )
        .unwrap();
        assert!(back.artifacts.is_empty());
    }

    #[test]
    fn import_error_display() {
        let e = ImportError::InvalidFormat {
            reason: "not a zip".into(),
        };
        assert!(format!("{e}").contains("not a zip"));
    }
}
