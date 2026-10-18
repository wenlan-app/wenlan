// SPDX-License-Identifier: AGPL-3.0-only
//! Opens a connected AI tool with a question already typed in its composer,
//! for the getting-started "ask your notes" step.
//!
//! Only first-party deep links built here can be opened, from a known tool
//! family and a percent-encoded question, so the frontend never hands the OS
//! an arbitrary URL. None of these links sends the question by itself: each
//! tool fills its composer and waits for the user to press send.

/// The link that opens `family` with `prompt` in a new chat, or `None` for a
/// tool that has no such link (command-line tools without a desktop app).
fn prompt_link(family: &str, prompt: &str) -> Option<String> {
    let q = percent_encode(prompt);
    match family {
        "claude-desktop" => Some(format!("claude://claude.ai/new?q={q}")),
        // Claude Code sessions also open in the Claude desktop app.
        "claude-code" => Some(format!("claude://code/new?q={q}")),
        // The Codex app reads the same ~/.codex setup as the Codex CLI.
        "codex" => Some(format!("codex://new?prompt={q}")),
        "cursor" => Some(format!(
            "cursor://anysphere.cursor-deeplink/prompt?text={q}"
        )),
        _ => None,
    }
}

/// RFC 3986 percent-encoding of everything outside the unreserved set, so the
/// question survives as one query value whatever it contains.
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// `Err` when the tool has no link or nothing on this machine opens it; the
/// caller has already copied the question, so the user can still paste it.
#[tauri::command]
pub async fn open_tool_with_prompt(family: String, prompt: String) -> Result<(), String> {
    let link = prompt_link(&family, &prompt).ok_or_else(|| format!("No link opens {family}"))?;
    // `open::that` waits for the OS opener, so an unregistered scheme comes
    // back as an error instead of failing silently.
    tokio::task::spawn_blocking(move || open::that(&link))
        .await
        .map_err(|e| format!("Failed to open: {e}"))?
        .map_err(|e| format!("Failed to open: {e}"))
}

#[cfg(test)]
mod tests {
    use super::{percent_encode, prompt_link};

    #[test]
    fn builds_each_known_tool_link_with_the_question_encoded() {
        let q = "我之前记过「A & B」吗？";
        let encoded = percent_encode(q);
        assert_eq!(
            prompt_link("codex", q).unwrap(),
            format!("codex://new?prompt={encoded}")
        );
        assert!(prompt_link("claude-desktop", q)
            .unwrap()
            .starts_with("claude://claude.ai/new?q="));
        assert!(prompt_link("claude-code", q)
            .unwrap()
            .starts_with("claude://code/new?q="));
        assert!(prompt_link("cursor", q)
            .unwrap()
            .starts_with("cursor://anysphere.cursor-deeplink/prompt?text="));
    }

    #[test]
    fn has_no_link_for_tools_without_one() {
        assert_eq!(prompt_link("gemini-cli", "hi"), None);
        assert_eq!(prompt_link("muse-code", "hi"), None);
        assert_eq!(prompt_link("https://example.com", "hi"), None);
    }

    #[test]
    fn encodes_everything_that_could_break_out_of_the_query_value() {
        assert_eq!(percent_encode("a b&c=d#e?f/g"), "a%20b%26c%3Dd%23e%3Ff%2Fg");
        assert_eq!(percent_encode("é"), "%C3%A9");
        assert_eq!(percent_encode("Az09-._~"), "Az09-._~");
    }
}
