// SPDX-License-Identifier: Apache-2.0
//! Pure tests for what a night may tidy (UX-GRAPH-7).

use super::*;

const NOW: i64 = 1_762_560_000; // 2025-11-08

fn item(text: &str, space: Option<&str>, created_at: i64) -> TidyItem {
    TidyItem {
        id: format!("mem_{created_at}"),
        text: text.to_string(),
        space: space.map(str::to_string),
        created_at,
        event_date: None,
        decided: false,
        session_id: None,
        line: None,
    }
}

fn items() -> Vec<TidyItem> {
    vec![
        item("The site stays free for now.", Some("wenlan"), NOW - 9_000),
        item("For now the site is free.", Some("wenlan"), NOW - 3_000),
        item("Pricing starts at 5 dollars.", Some("wenlan"), NOW - 90_000),
        item(
            "There is no paid plan; it stays free.",
            Some("wenlan"),
            NOW - 1_000,
        ),
        TidyItem {
            event_date: Some(NOW - 86_400 * 5),
            ..item(
                "The launch is set for Nov 3.",
                Some("wenlan"),
                NOW - 86_400 * 9,
            )
        },
        item("The site stays free for now.", Some("personal"), NOW - 500),
    ]
}

#[test]
fn the_prompt_shows_each_group_with_its_dates() {
    let items = items();
    let prompt = tidy_prompt(&items, &[vec![0, 1], vec![4]], NOW);
    assert!(prompt.contains("Today is Nov 8, 2025."), "{prompt}");
    assert!(prompt.contains("Group 1:\nM0 (written"), "{prompt}");
    assert!(
        prompt.contains("M4 (written Oct 30, 2025, about Nov 3, 2025)"),
        "{prompt}"
    );
    assert!(
        !prompt.contains("Pricing starts"),
        "only the groups' memories are shown"
    );
}

#[test]
fn a_night_keeps_only_changes_that_pass_the_checks() {
    let items = items();
    let answer = r#"{"changes": [
        {"do": "merge", "memories": ["M0", "M1"], "text": "The site stays free for now."},
        {"do": "replace", "keep": "M3", "drop": "M2"},
        {"do": "update", "memory": "M4", "text": "The launch was set for Nov 3."}
    ]}"#;
    let changes = parse_tidy(answer, &items, NOW).unwrap();
    assert_eq!(
        changes,
        [
            TidyChange::Merge {
                items: [0, 1],
                text: "The site stays free for now.".into()
            },
            TidyChange::Replace { keep: 3, drop: 2 },
            TidyChange::Update {
                item: 4,
                text: "The launch was set for Nov 3.".into()
            },
        ]
    );
}

#[test]
fn a_change_that_could_hide_a_fact_or_make_one_up_is_dropped() {
    let items = items();
    let answer = r#"{"changes": [
        {"do": "merge", "memories": ["M0", "M5"], "text": "The site stays free."},
        {"do": "merge", "memories": ["M0", "M1"], "text": "The site stays free until 2027."},
        {"do": "replace", "keep": "M2", "drop": "M3"},
        {"do": "update", "memory": "M0", "text": "The site was free."},
        {"do": "update", "memory": "M4", "text": "The launch on Nov 3 drew 40 people."},
        {"do": "merge", "memories": ["M9", "M1"], "text": "x"},
        {"do": "merge", "memories": ["M0", "M1"], "text": "See PR #1202."},
        {"do": "delete", "memory": "M1"}
    ]}"#;
    let changes = parse_tidy(answer, &items, NOW).unwrap();
    // Across Spaces; a number neither says; the older kept over the newer;
    // a memory with no passed date; an outcome nobody wrote; a memory not
    // offered; not plain words; and an action there is no such thing as.
    assert!(changes.is_empty(), "{changes:?}");
}

#[test]
fn each_memory_is_changed_once_and_numbers_compare_by_value() {
    let mut items = items();
    items[0].text = "The review is on 2025-11-03 at 09:00.".into();
    items[1].text = "Review set for Nov 3 at 9.".into();
    let answer = r#"{"changes": [
        {"do": "merge", "memories": ["M0", "M1"], "text": "The review is on Nov 3 at 9."},
        {"do": "replace", "keep": "M1", "drop": "M0"}
    ]}"#;
    let changes = parse_tidy(answer, &items, NOW).unwrap();
    assert_eq!(changes.len(), 1, "{changes:?}");
    assert!(parse_tidy("not json", &items, NOW).is_none());
}

#[test]
fn a_date_has_passed_only_once_its_whole_day_is_over() {
    let day = 1_762_560_000; // 2025-11-08 00:00 UTC
    assert!(!date_passed(day, day + 3_600), "early that day, anywhere");
    assert!(!date_passed(day, day + 86_399));
    assert!(date_passed(day, day + 86_400));
}
