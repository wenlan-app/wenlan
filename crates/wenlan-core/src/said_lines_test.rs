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
