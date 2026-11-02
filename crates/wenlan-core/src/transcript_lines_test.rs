// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::conversations::ConversationSpeaker::{Ai, You};

const TRANSCRIPT: &str = r#"{"type":"ai-title","aiTitle":"Kyoto trip"}
{"type":"user","isMeta":true,"message":{"role":"user","content":"<session-context>ignored</session-context>"},"timestamp":"2026-11-01T10:00:00Z"}
{"type":"user","message":{"role":"user","content":"Should we stay near Gion?"},"timestamp":"2026-11-01T10:00:05Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"thinking","thinking":"private"},{"type":"text","text":"Quiet, or easy to walk out at night?"}]},"timestamp":"2026-11-01T10:00:09Z"}
{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{}}]},"timestamp":"2026-11-01T10:00:10Z"}
{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"file body"}]},"timestamp":"2026-11-01T10:00:11Z"}
{"type":"user","message":{"role":"user","content":"<command-name>/clear</command-name>"},"timestamp":"2026-11-01T10:00:12Z"}
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"[Request interrupted by user]"}]},"timestamp":"2026-11-01T10:00:13Z"}
{"type":"user","isSidechain":true,"message":{"role":"user","content":"subagent prompt"},"timestamp":"2026-11-01T10:00:14Z"}
not json at all
{"type":"user","message":{"role":"user","content":[{"type":"text","text":"Decided: stay near Gion, evening walks are easy."}]},"timestamp":"2026-11-01T10:01:00Z"}
"#;

fn lines() -> Vec<ConversationLine> {
    parse_claude_code(TRANSCRIPT.as_bytes())
}

#[test]
fn ux_find_2_keeps_only_what_the_person_and_the_ai_said() {
    let lines = lines();
    let said: Vec<(ConversationSpeaker, &str)> =
        lines.iter().map(|l| (l.speaker, l.text.as_str())).collect();
    assert_eq!(
        said,
        vec![
            (You, "Should we stay near Gion?"),
            (Ai, "Quiet, or easy to walk out at night?"),
            (You, "Decided: stay near Gion, evening walks are easy."),
        ]
    );
    assert_eq!(lines[0].at, Some(1_793_527_205));
}

#[test]
fn ux_find_1_matches_every_word_ignoring_case() {
    let terms = search_terms("  gion   DECIDED ");
    assert_eq!(terms, vec!["gion", "decided"]);
    let hits: Vec<usize> = lines()
        .iter()
        .enumerate()
        .filter(|(_, l)| line_matches(&l.text, &terms))
        .map(|(i, _)| i)
        .collect();
    assert_eq!(hits, vec![2]);
    assert!(!line_matches("anything", &[]));
    assert!(line_matches("决定住在祇园附近", &search_terms("祇园")));
}

#[test]
fn ux_find_2_excerpts_centre_on_the_word_and_mark_cuts() {
    let text = format!("{}祇园{}", "前".repeat(100), "后".repeat(100));
    let cut = excerpt(&text, &search_terms("祇园"), 30);
    assert!(cut.starts_with('…') && cut.ends_with('…'), "{cut}");
    assert!(cut.contains("祇园"), "{cut}");
    assert_eq!(cut.chars().count(), 32);
    assert_eq!(excerpt("short", &search_terms("x"), 30), "short");
    assert_eq!(head("abcdef", 3), "abc…");
    assert_eq!(tail("abcdef", 3), "…def");
}

#[test]
fn ux_find_2_reads_a_transcript_file_once_until_it_changes() {
    let dir = std::env::temp_dir().join(format!("wenlan-lines-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.jsonl");
    std::fs::write(&path, TRANSCRIPT).unwrap();
    let first = conversation_lines("claude-code", &path).unwrap();
    assert_eq!(first.len(), 3);
    let again = conversation_lines("claude-code", &path).unwrap();
    assert!(
        Arc::ptr_eq(&first, &again),
        "unchanged file is not read again"
    );
    std::fs::write(
        &path,
        format!(
            "{TRANSCRIPT}{}\n",
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Noted."}]}}"#
        ),
    )
    .unwrap();
    assert_eq!(conversation_lines("claude-code", &path).unwrap().len(), 4);
    assert!(
        conversation_lines("codex", &path).is_none(),
        "unknown format"
    );
    std::fs::remove_file(&path).unwrap();
    assert!(
        conversation_lines("claude-code", &path).is_none(),
        "pruned file"
    );
    std::fs::remove_dir(&dir).ok();
}
