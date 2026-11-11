// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::{BriefItem, BriefItemState};

fn brief(items: &[(&str, &str)]) -> Brief {
    Brief {
        space_id: "sp1".into(),
        space: "wenlan".into(),
        last_session_summary: "Trust audit fixes all merged (last: #1236).".into(),
        last_handoff_at: Some(1_794_000_000),
        version: 7,
        active: items
            .iter()
            .map(|(id, text)| BriefItem {
                id: (*id).into(),
                text: (*text).into(),
                state: BriefItemState::Active,
                added_at: 1_790_000_000,
                gate: None,
                version: 1,
            })
            .collect(),
        backlog: vec![],
    }
}

#[test]
fn ux_space_2_prompt_asks_for_plain_lines_in_the_language() {
    let b = brief(&[("a", "Merge PR #1210 (share a Space, head 967c4c6)")]);
    let prompt = plain_prompt(&b, &["#1210 merged on 11-08".into()], "Traditional Chinese");
    assert!(prompt.contains("I0: Merge PR #1210"));
    assert!(prompt.contains("in Traditional Chinese"));
    assert!(prompt.contains("- #1210 merged on 11-08"));
    assert!(prompt.contains("no PR or issue numbers, version numbers"));
}

#[test]
fn ux_space_2_keeps_plain_short_lines_and_drops_the_rest() {
    let b = brief(&[("a", "x"), ("b", "y"), ("c", "z")]);
    let answer = r#"{"summary": "可信度檢查的修正都合併了。",
        "items": [{"item": "I0", "line": "Windows 程式碼簽章還沒解決"},
                  {"item": "I1", "line": "合併 #1210"},
                  {"item": "I2", "line": "這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長這一行非常非常長"},
                  {"item": "I9", "line": "不存在"}]}"#;
    let parsed = parse_plain(answer, &b, &[]).unwrap();
    assert_eq!(
        parsed.summary.as_deref(),
        Some("可信度檢查的修正都合併了。")
    );
    assert_eq!(parsed.lines.len(), 1);
    assert_eq!(parsed.lines["a"], "Windows 程式碼簽章還沒解決");
    assert!(parsed.done.is_empty());
}

#[test]
fn ux_space_2_closes_only_with_a_quote_found_in_the_evidence() {
    let b = brief(&[
        ("a", "Merge the share PR"),
        ("b", "Sign Windows builds"),
        ("c", "Ship film"),
    ]);
    let evidence = vec![
        "Sharing a Space through a folder   was merged and released.".to_string(),
        "Film still waiting".to_string(),
    ];
    let answer = r#"{"summary": "", "items": [{"item": "I0", "line": "Share PR"}, {"item": "I1", "line": "Sign Windows builds"}],
        "done": [{"item": "I0", "quote": "through a folder was merged"},
                 {"item": "I1", "quote": "Windows signing is done"},
                 {"item": "I2", "quote": "Film"}]}"#;
    let parsed = parse_plain(answer, &b, &evidence).unwrap();
    assert_eq!(
        parsed.done,
        vec![Done {
            item_id: "a".into(),
            quote: "through a folder was merged".into()
        }]
    );
    assert!(
        !parsed.lines.contains_key("a"),
        "a closed item gets no line"
    );
    assert_eq!(parsed.lines["b"], "Sign Windows builds");
    assert_eq!(parsed.summary, None);
}

#[test]
fn ux_space_2_not_json_is_none_and_think_is_skipped() {
    let b = brief(&[("a", "x")]);
    assert!(parse_plain("no idea", &b, &[]).is_none());
    let parsed = parse_plain(
        r#"<think>{"items": []}</think>{"items": [{"item": "I0", "line": "Do x"}]}"#,
        &b,
        &[],
    )
    .unwrap();
    assert_eq!(parsed.lines["a"], "Do x");
}

#[test]
fn ux_space_2_lines_are_rewritten_for_a_new_version_or_language() {
    let b = brief(&[("a", "x")]);
    let plain = BriefPlain {
        brief_version: 7,
        language: "English".into(),
        ..Default::default()
    };
    assert!(is_current(Some(&plain), &b, "English"));
    assert!(!is_current(Some(&plain), &b, "Traditional Chinese"));
    assert!(!is_current(
        Some(&BriefPlain {
            brief_version: 6,
            ..plain.clone()
        }),
        &b,
        "English"
    ));
    assert!(!is_current(None, &b, "English"));
    assert_eq!(metadata_key("sp1"), "brief_plain_v1:sp1");
}
