// SPDX-License-Identifier: Apache-2.0
use super::*;
use wenlan_types::conversations::ConversationSpeaker::{self, Ai, You};

fn line(speaker: ConversationSpeaker, text: &str, at: i64) -> ConversationLine {
    ConversationLine {
        speaker,
        text: text.to_string(),
        at: Some(at),
    }
}

#[test]
fn ux_who_6_picks_the_line_that_says_the_same_thing() {
    let lines = vec![
        line(You, "Should we stay near Gion or near the station?", 100),
        line(Ai, "Near the station is quicker for day trips.", 105),
        line(You, "Decided: stay near Gion, evening walks are easy.", 160),
        line(Ai, "Noted.", 164),
    ];
    let picked = said_lines("Stay near Gion: evening walks are easy.", 170, &lines);
    assert!(picked.contains(&2), "{picked:?}");
    assert!(!picked.contains(&1) && !picked.contains(&3), "{picked:?}");
}

#[test]
fn ux_who_6_reads_chinese_by_character_pairs() {
    let lines = vec![
        line(You, "0.21 要分幾次發？", 100),
        line(You, "那就一次發 0.21，等 Zotero 那幾個 PR 進來。", 160),
        line(Ai, "好的。", 164),
    ];
    let picked = said_lines("決定一次發 0.21，等 Zotero 的 PR。", 170, &lines);
    assert!(picked.contains(&1), "{picked:?}");
    assert!(!picked.contains(&2), "{picked:?}");
}

#[test]
fn ux_who_6_never_looks_after_the_memory_was_saved() {
    let lines = vec![
        line(You, "Decided: stay near Gion, evening walks are easy.", 100),
        line(
            You,
            "Decided: stay near Gion, evening walks are easy.",
            100 + 3_600,
        ),
    ];
    assert_eq!(
        said_lines("Stay near Gion, evening walks are easy.", 100, &lines),
        vec![0]
    );
    // A reply written a moment after the save still counts.
    let late = vec![line(
        Ai,
        "Saved: stay near Gion, evening walks are easy.",
        100 + SAVE_SLACK_SECS,
    )];
    assert_eq!(
        said_lines("Stay near Gion, evening walks are easy.", 100, &late),
        vec![0]
    );
}

#[test]
fn ux_who_6_says_nothing_rather_than_guess() {
    let lines = vec![
        line(You, "What time is dinner?", 100),
        line(Ai, "Seven.", 105),
    ];
    assert!(said_lines("Stay near Gion, evening walks are easy.", 200, &lines).is_empty());
    assert!(said_lines("", 200, &lines).is_empty());
    let untimed = vec![ConversationLine {
        speaker: You,
        text: "Stay near Gion, evening walks are easy.".into(),
        at: None,
    }];
    assert!(said_lines("Stay near Gion, evening walks are easy.", 200, &untimed).is_empty());
}

#[test]
fn ux_who_6_picks_at_most_three_oldest_first() {
    let lines: Vec<_> = (0..6)
        .map(|i| line(You, "stay near Gion, evening walks are easy", 100 + i))
        .collect();
    // Ties go to the lines nearest the save, then they are shown in order.
    assert_eq!(
        said_lines("stay near Gion, evening walks are easy", 200, &lines),
        vec![3, 4, 5]
    );
}

#[test]
fn ux_who_6_a_long_reply_does_not_win_by_length() {
    let filler: String = (0..400).map(|i| format!("topic{i} ")).collect();
    let lines = vec![
        line(You, "Decided: stay near Gion, evening walks are easy.", 100),
        line(
            Ai,
            &format!("{filler} stay near Gion evening walks easy"),
            105,
        ),
    ];
    assert_eq!(
        said_lines("Stay near Gion, evening walks are easy.", 200, &lines),
        vec![0]
    );
}

/// Trust audit F7: a night's point keeps the line it came from, so the line
/// is found by what was typed, not by shared words with a paraphrase.
#[test]
fn ux_who_6_a_nights_point_finds_the_line_it_named() {
    let lines = vec![
        line(Ai, "keep the resume to one page?", 90),
        line(You, "ok   keep it to one page,\nsame format as before", 100),
        line(You, "and centre the header", 110),
    ];
    assert_eq!(
        quoted_line("ok keep it to one page, same format as before", &lines),
        Some(1)
    );
    assert_eq!(quoted_line("never said", &lines), None);
    assert_eq!(quoted_line("  ", &lines), None);
}

/// A web chat has no transcript Wenlan can read, so the AI tool sends the
/// lines with the memory; Wenlan keeps a few, trimmed, stamped with the save.
#[test]
fn ux_who_6_kept_lines_are_trimmed_capped_and_stamped_with_the_save() {
    use wenlan_types::conversations::{MAX_KEPT_SAID_CHARS, MAX_KEPT_SAID_LINES};
    let long = "x".repeat(MAX_KEPT_SAID_CHARS + 10);
    let sent = vec![
        line(You, "first, too far back", 1),
        line(Ai, "  a reply  ", 2),
        line(You, " \n ", 3),
        line(You, "the decision", 4),
        line(Ai, &long, 5),
    ];
    let kept = kept_said(&sent, 500);
    assert_eq!(kept.len(), MAX_KEPT_SAID_LINES);
    assert_eq!(kept[0].text, "a reply");
    assert_eq!(kept[1].text, "the decision");
    assert_eq!(kept[1].speaker, You);
    assert_eq!(kept[2].text.chars().count(), MAX_KEPT_SAID_CHARS);
    assert!(kept.iter().all(|l| l.at == Some(500)));
    assert!(kept_said(&[line(You, "   ", 1)], 500).is_empty());
}
