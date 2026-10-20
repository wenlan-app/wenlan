use super::*;

fn span(start: u32, end: u32) -> LineSpan {
    LineSpan { start, end }
}

/// `n` distinct lines `prefix 1` .. `prefix n`.
fn numbered(prefix: &str, n: u32) -> Vec<String> {
    (1..=n).map(|i| format!("{prefix} {i}")).collect()
}

fn text(lines: &[String]) -> String {
    let mut out = lines.join("\n");
    out.push('\n');
    out
}

/// A 30-line file with distinct lines.
fn base_lines() -> Vec<String> {
    numbered("line", 30)
}

#[test]
fn empty_text_and_empty_spans_are_errors() {
    assert_eq!(
        anchor("", span(1, 1)),
        Err(AnchorError::SpanOutOfRange { line_count: 0 })
    );
    assert_eq!(anchor("a\nb\n", span(2, 1)), Err(AnchorError::EmptySpan));
    assert_eq!(anchor("a\nb\n", span(0, 1)), Err(AnchorError::EmptySpan));
}

#[test]
fn span_past_end_of_file_is_an_error() {
    assert_eq!(
        anchor("a\nb\nc\n", span(2, 4)),
        Err(AnchorError::SpanOutOfRange { line_count: 3 })
    );
    assert_eq!(
        anchor("a\nb\nc\n", span(4, 4)),
        Err(AnchorError::SpanOutOfRange { line_count: 3 })
    );
}

#[test]
fn trailing_newline_is_not_a_phantom_line() {
    assert!(anchor("a\nb\n", span(2, 2)).is_ok());
    assert!(anchor("a\nb", span(2, 2)).is_ok());
    assert_eq!(
        anchor("a\nb\n", span(3, 3)),
        Err(AnchorError::SpanOutOfRange { line_count: 2 })
    );
    // A blank line is a real line.
    assert!(anchor("a\n\n", span(2, 2)).is_ok());
}

#[test]
fn single_line_span_hashes_the_line() {
    let a = anchor(&text(&base_lines()), span(15, 15)).unwrap();
    assert_eq!(a.selected_line_count, 1);
    assert_eq!(a.first_line_hash, a.last_line_hash);
    assert_eq!(a.digest, a.first_line_hash);
    assert_eq!(a.preceding_context_line_count, CONTEXT_LINES);
    assert_eq!(a.following_context_line_count, CONTEXT_LINES);
    assert_eq!(a.digest.len(), 64);
}

#[test]
fn span_at_file_start_has_no_preceding_context() {
    let a = anchor(&text(&base_lines()), span(1, 4)).unwrap();
    assert_eq!(a.preceding_context_line_count, 0);
    assert_eq!(a.following_context_line_count, CONTEXT_LINES);
}

#[test]
fn span_near_file_edges_has_short_context() {
    let a = anchor(&text(&base_lines()), span(2, 4)).unwrap();
    assert_eq!(a.preceding_context_line_count, 1);
    let a = anchor(&text(&base_lines()), span(27, 29)).unwrap();
    assert_eq!(a.following_context_line_count, 1);
}

#[test]
fn span_at_file_end_has_no_following_context() {
    let a = anchor(&text(&base_lines()), span(27, 30)).unwrap();
    assert_eq!(a.following_context_line_count, 0);
    assert_eq!(a.preceding_context_line_count, CONTEXT_LINES);
}

#[test]
fn unchanged_file_is_unchanged_at_the_hint() {
    let t = text(&base_lines());
    let a = anchor(&t, span(10, 20)).unwrap();
    assert_eq!(
        relocate(&t, Some(span(10, 20)), &a),
        Relocation::Unchanged { span: span(10, 20) }
    );
}

#[test]
fn without_a_hint_an_intact_span_is_reported_as_moved() {
    // No hint: the unique match is a move from "nowhere", never Unchanged.
    let t = text(&base_lines());
    let a = anchor(&t, span(10, 20)).unwrap();
    assert_eq!(
        relocate(&t, None, &a),
        Relocation::Moved { span: span(10, 20) }
    );
}

#[test]
fn edge_selections_stay_unchanged() {
    let t = text(&base_lines());
    for s in [span(1, 3), span(28, 30), span(1, 30)] {
        let a = anchor(&t, s).unwrap();
        assert_eq!(relocate(&t, Some(s), &a), Relocation::Unchanged { span: s });
    }
}

#[test]
fn inserting_lines_above_moves_the_span() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines.insert(0, "// new header comment".to_string());
    lines.insert(0, "// another".to_string());
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Moved { span: span(12, 22) }
    );
}

#[test]
fn deleting_lines_above_moves_the_span_up() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines.drain(0..4);
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Moved { span: span(6, 16) }
    );
}

#[test]
fn editing_a_line_inside_is_changed_with_the_same_span() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines[14] = "line 15 but edited".to_string();
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Changed { span: span(10, 20) }
    );
}

#[test]
fn editing_the_first_and_last_selected_lines_is_still_changed() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines[9] = "different start".to_string();
    lines[19] = "different end".to_string();
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Changed { span: span(10, 20) }
    );
}

#[test]
fn edit_plus_insert_above_is_changed_at_the_new_place() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines[14] = "edited".to_string();
    lines.insert(0, "// header".to_string());
    // Stale hint: the span the note remembered.
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Changed { span: span(11, 21) }
    );
}

#[test]
fn growing_the_region_is_changed_with_a_longer_span() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines.insert(14, "an added line".to_string());
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Changed { span: span(10, 21) }
    );
}

#[test]
fn changed_region_at_file_start_and_end_is_fenced_by_the_file_edge() {
    let original = text(&base_lines());

    let a = anchor(&original, span(1, 5)).unwrap();
    let mut lines = base_lines();
    lines[2] = "edited".to_string();
    assert_eq!(
        relocate(&text(&lines), Some(span(1, 5)), &a),
        Relocation::Changed { span: span(1, 5) }
    );

    let a = anchor(&original, span(26, 30)).unwrap();
    let mut lines = base_lines();
    lines[27] = "edited".to_string();
    assert_eq!(
        relocate(&text(&lines), Some(span(26, 30)), &a),
        Relocation::Changed { span: span(26, 30) }
    );
}

#[test]
fn deleting_the_whole_region_is_unresolved() {
    let original = text(&base_lines());
    let a = anchor(&original, span(10, 20)).unwrap();
    let mut lines = base_lines();
    lines.drain(9..20);
    assert_eq!(
        relocate(&text(&lines), Some(span(10, 20)), &a),
        Relocation::Unresolved
    );
}

#[test]
fn deleting_a_region_at_the_file_edge_is_unresolved() {
    let original = text(&base_lines());
    let a = anchor(&original, span(1, 5)).unwrap();
    let mut lines = base_lines();
    lines.drain(0..5);
    assert_eq!(
        relocate(&text(&lines), Some(span(1, 5)), &a),
        Relocation::Unresolved
    );
    // And an emptied file.
    assert_eq!(relocate("", Some(span(1, 5)), &a), Relocation::Unresolved);
}

/// Two copies of a 4-line block, each with different surroundings.
fn duplicated_block_file() -> (Vec<String>, LineSpan, LineSpan) {
    let block = ["fn dup() {", "    work();", "    more();", "}"];
    let mut lines = numbered("head", 5);
    lines.extend(block.iter().map(|s| s.to_string()));
    lines.extend(numbered("middle", 5));
    lines.extend(block.iter().map(|s| s.to_string()));
    lines.extend(numbered("tail", 5));
    (lines, span(6, 9), span(15, 18))
}

#[test]
fn duplicated_block_without_a_hint_is_unresolved_when_context_also_matches_twice() {
    // Same block *and* same surroundings twice: truly ambiguous.
    let block = ["fn dup() {", "    work();", "}"];
    let mut lines = Vec::new();
    for _ in 0..2 {
        lines.extend(["a", "b", "c"].map(String::from));
        lines.extend(block.map(String::from));
        lines.extend(["x", "y", "z"].map(String::from));
    }
    let t = text(&lines);
    let a = anchor(&t, span(4, 6)).unwrap();
    assert_eq!(relocate(&t, None, &a), Relocation::Unresolved);
}

#[test]
fn duplicated_block_with_distinct_context_is_resolved() {
    let (lines, first, second) = duplicated_block_file();
    let t = text(&lines);
    for target in [first, second] {
        let a = anchor(&t, target).unwrap();
        assert_eq!(
            relocate(&t, None, &a),
            Relocation::Moved { span: target },
            "no hint: found by context"
        );
        assert_eq!(
            relocate(&t, Some(target), &a),
            Relocation::Unchanged { span: target }
        );
    }
}

#[test]
fn duplicated_block_follows_its_own_copy_after_an_insert_above() {
    let (mut lines, _, second) = duplicated_block_file();
    let a = anchor(&text(&lines), second).unwrap();
    lines.insert(0, "// shifted".to_string());
    assert_eq!(
        relocate(&text(&lines), Some(second), &a),
        Relocation::Moved {
            span: span(second.start + 1, second.end + 1)
        }
    );
}

#[test]
fn crlf_and_trailing_space_edits_are_unchanged() {
    let lines = base_lines();
    let original = text(&lines);
    let a = anchor(&original, span(10, 20)).unwrap();

    let crlf = original.replace('\n', "\r\n");
    assert_eq!(
        relocate(&crlf, Some(span(10, 20)), &a),
        Relocation::Unchanged { span: span(10, 20) }
    );

    let old_mac = original.replace('\n', "\r");
    assert_eq!(
        relocate(&old_mac, Some(span(10, 20)), &a),
        Relocation::Unchanged { span: span(10, 20) }
    );

    let trailing: String = lines.iter().map(|l| format!("{l}  \t\n")).collect();
    assert_eq!(
        relocate(&trailing, Some(span(10, 20)), &a),
        Relocation::Unchanged { span: span(10, 20) }
    );

    // No final newline is also fine.
    let no_final = original.trim_end_matches('\n').to_string();
    assert_eq!(
        relocate(&no_final, Some(span(10, 20)), &a),
        Relocation::Unchanged { span: span(10, 20) }
    );
}

#[test]
fn anchors_do_not_depend_on_line_endings_or_trailing_space() {
    let lf = "a\nb\nc\n";
    let messy = "a  \r\nb\t\r\nc";
    assert_eq!(
        anchor(lf, span(1, 3)).unwrap(),
        anchor(messy, span(1, 3)).unwrap()
    );
}

#[test]
fn indentation_changes_are_changes() {
    let lines = vec![
        "fn a() {".to_string(),
        "    body();".to_string(),
        "}".to_string(),
    ];
    let a = anchor(&text(&lines), span(1, 3)).unwrap();
    let mut edited = lines.clone();
    edited[1] = "  body();".to_string();
    assert_eq!(
        relocate(&text(&edited), Some(span(1, 3)), &a),
        Relocation::Changed { span: span(1, 3) }
    );
}

#[test]
fn hint_outside_the_file_is_ignored() {
    let t = text(&base_lines());
    let a = anchor(&t, span(10, 20)).unwrap();
    assert_eq!(
        relocate(&t, Some(span(90, 99)), &a),
        Relocation::Moved { span: span(10, 20) }
    );
    assert_eq!(
        relocate(&t, Some(span(0, 3)), &a),
        Relocation::Moved { span: span(10, 20) }
    );
}

#[test]
fn degenerate_anchor_is_unresolved() {
    let t = text(&base_lines());
    let mut a = anchor(&t, span(10, 20)).unwrap();
    a.selected_line_count = 0;
    assert_eq!(relocate(&t, None, &a), Relocation::Unresolved);
}

#[test]
fn anchor_serde_round_trip() {
    let a = anchor(&text(&base_lines()), span(10, 20)).unwrap();
    let json = serde_json::to_string(&a).unwrap();
    assert!(json.contains("\"digest\""));
    assert!(json.contains("\"preceding_context_hash\""));
    let back: CodeAnchor = serde_json::from_str(&json).unwrap();
    assert_eq!(back, a);
}
