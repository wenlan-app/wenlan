// SPDX-License-Identifier: Apache-2.0
//! Pure tests for sentence authorship and the sentence write gate.

use super::*;
use wenlan_types::authorship::SentenceAuthor::{Ai, Original, Person, Unknown};

fn v<'a>(version: i64, edited_by: &'a str, content: &'a str) -> VersionText<'a> {
    VersionText {
        version,
        edited_by,
        content,
    }
}

fn authors(rows: &[SentenceAuthorship]) -> Vec<SentenceAuthor> {
    rows.iter().map(|r| r.author).collect()
}

fn derive(kind: &str, versions: &[VersionText<'_>]) -> Vec<SentenceAuthorship> {
    derive_authorship("p", kind, versions, &HashMap::new(), false)
}

#[test]
fn units_split_lines_then_sentences_and_ignore_markers_and_wrapping() {
    let body = "# Title\n\nFirst claim.[1] Second claim.\n\n- A list item\n---\n";
    let units = sentence_units(body);
    let texts: Vec<&str> = units.iter().map(|u| u.text.as_str()).collect();
    assert_eq!(
        texts,
        [
            "# Title",
            "First claim.[1]",
            "Second claim.",
            "- A list item"
        ]
    );
    assert_eq!(&body[units[1].start..units[1].end], "First claim.[1]");
    assert_eq!(units[1].norm, "First claim.");
    assert!(units[0].whole_line && !units[1].whole_line && units[3].whole_line);
    assert!(units[1].blank_before);
    assert_eq!(
        sentence_hash(&normalize_sentence("First   claim.[2]")),
        units[1].hash,
        "re-citing or re-spacing a sentence keeps its hash"
    );
}

#[test]
fn a_person_touch_up_makes_the_sentence_theirs_but_keeps_its_ai_origin() {
    let rows = derive(
        "distilled",
        &[
            v(1, "create", "The launch is in March. The team is small."),
            v(
                2,
                "manual_edit",
                "The launch is in April. The team is small.",
            ),
        ],
    );
    assert_eq!(authors(&rows), [Person, Ai]);
    assert_eq!(
        rows[0].origin, Ai,
        "a touch-up keeps the citation requirement"
    );
    assert_eq!(rows[0].since_version, 2);
    assert_eq!(rows[1].since_version, 1);
    assert_eq!(rows[1].version, 2);
}

#[test]
fn a_new_sentence_from_the_person_has_person_origin_and_an_ai_rewrite_takes_it_back() {
    let rows = derive(
        "distilled",
        &[
            v(1, "create", "AI wrote this."),
            v(
                2,
                "manual_edit",
                "AI wrote this. Completely different words here.",
            ),
        ],
    );
    assert_eq!(authors(&rows), [Ai, Person]);
    assert_eq!(rows[1].origin, Person);

    let rows = derive(
        "distilled",
        &[
            v(1, "create", "Mine now."),
            v(2, "manual_edit", "Mine now, really."),
            v(3, "agent_refresh", "The agent rewrote it."),
        ],
    );
    assert_eq!(
        authors(&rows),
        [Ai],
        "a direct AI edit is recorded as AI-written"
    );
}

#[test]
fn migration_87_seed_is_unknown_and_protected_only_on_guarded_pages() {
    let rows = derive(
        "distilled",
        &[
            v(1, "migration_87", "Old text. More old text."),
            v(2, "distill", "Old text. A fresh AI line."),
        ],
    );
    assert_eq!(authors(&rows), [Unknown, Ai]);
    assert_eq!(rows[0].confidence, AuthorshipConfidence::Uncertain);
    assert!(is_protected(Unknown, true));
    assert!(!is_protected(Unknown, false));
    assert!(is_protected(Person, false));
    assert!(is_protected(Original, false));
    assert!(!is_protected(Ai, true));
}

#[test]
fn moved_and_rewrapped_sentences_keep_their_author() {
    let rows = derive(
        "authored",
        &[
            v(1, "create", "Person one.\nPerson two."),
            v(2, "distill", "Person two.\nAI added this.\nPerson   one."),
        ],
    );
    assert_eq!(authors(&rows), [Person, Ai, Person]);
}

#[test]
fn a_link_rewrite_keeps_the_author_of_the_sentence_it_touched() {
    let rows = derive(
        "authored",
        &[
            v(1, "create", "See [[Old name]] for details."),
            v(2, "link_rename", "See [[New name]] for details."),
        ],
    );
    assert_eq!(authors(&rows), [Person]);
}

#[test]
fn accepted_proposals_are_the_persons_with_ai_origin_and_unknown_writers_are_uncertain() {
    let accepted_text = "The AI suggested this.";
    let hash = sentence_hash(&normalize_sentence(accepted_text));
    let accepted = HashMap::from([(2, HashSet::from([hash]))]);
    let rows = derive_authorship(
        "p",
        "authored",
        &[
            v(1, "create", "Something else entirely here."),
            v(2, "manual_edit", accepted_text),
            v(3, "someone_new", "The AI suggested this. Mystery line."),
        ],
        &accepted,
        false,
    );
    assert_eq!(authors(&rows), [Person, Unknown]);
    assert!(rows[0].accepted);
    assert_eq!(rows[0].origin, Ai);
    assert_eq!(rows[1].confidence, AuthorshipConfidence::Uncertain);
}

#[test]
fn a_truncated_history_starts_from_unknown() {
    let rows = derive_authorship(
        "p",
        "distilled",
        &[
            v(40, "distill", "Old line."),
            v(41, "distill", "Old line. New line."),
        ],
        &HashMap::new(),
        true,
    );
    assert_eq!(authors(&rows), [Unknown, Ai]);
}

#[test]
fn imported_original_text_is_never_the_persons() {
    assert_eq!(
        version_author("create", "original"),
        VersionAuthor::Original
    );
    assert_eq!(
        version_author("chat_import", "distilled"),
        VersionAuthor::Original
    );
    assert_eq!(version_author("create", "authored"), VersionAuthor::Person);
    assert_eq!(
        version_author("migration_87", "authored"),
        VersionAuthor::Unknown
    );
}

#[test]
fn the_gate_keeps_a_rewritten_person_sentence_and_lands_the_ai_change() {
    let current = "Mine stays.[1] The AI line.[2]\n";
    let new = "Mine rewritten by AI.[1] The AI line, better.[2]\n";
    let protected = HashSet::from([0]);
    let plan = plan_sentence_gate(current, new, &protected).expect("a protected change");
    assert_eq!(plan.content, "Mine stays. The AI line, better.[2]\n");
    assert_eq!(
        plan.proposals,
        [PlannedProposal {
            sentence_hash: sentence_hash("Mine stays."),
            before: "Mine stays.[1]".into(),
            after: "Mine rewritten by AI.".into(),
        }]
    );
    assert_eq!(plan.kept_occurrences, [2]);
}

#[test]
fn the_gate_puts_back_a_deleted_person_line_and_passes_untouched_writes() {
    let current = "# Notes\n\nMy own line.\n\nAI paragraph.\n";
    let new = "# Notes\n\nAI paragraph, rewritten.\n";
    let protected = HashSet::from([0, 1]);
    let plan = plan_sentence_gate(current, new, &protected).expect("a deletion of mine");
    assert_eq!(
        plan.content,
        "# Notes\n\nMy own line.\n\nAI paragraph, rewritten.\n"
    );
    assert_eq!(plan.proposals.len(), 1);
    assert_eq!(plan.proposals[0].after, "");

    let untouched = "# Notes\n\nMy own line.\n\nAI paragraph, rewritten.\n";
    assert_eq!(plan_sentence_gate(current, untouched, &protected), None);
    assert_eq!(plan_sentence_gate(current, new, &HashSet::new()), None);
}

#[test]
fn the_gate_treats_a_moved_person_sentence_as_kept() {
    let current = "Mine.\nAI one.\n";
    let new = "AI one.\nMine.\n";
    assert_eq!(plan_sentence_gate(current, new, &HashSet::from([0])), None);
}

#[test]
fn remapped_citations_follow_the_markers_that_survived() {
    let json = serde_json::json!([
        {"occurrence":1,"marker":1,"source_kind":"memory","locator":"a","score":1.0,"status":"verified"},
        {"occurrence":2,"marker":2,"source_kind":"memory","locator":"b","score":1.0,"status":"verified"}
    ])
    .to_string();
    let remapped: Vec<wenlan_types::pages::PageCitation> =
        serde_json::from_str(&remap_citations(&json, &[2]).unwrap()).unwrap();
    assert_eq!(remapped.len(), 1);
    assert_eq!(remapped[0].locator, "b");
    assert_eq!(remapped[0].occurrence, 1);
    assert_eq!(remap_citations("not json", &[1]), None);
}

#[test]
fn alignment_pairs_only_touch_ups_as_replacements() {
    let ops = align(
        &["a b c", "keep this one", "z"],
        &["a b c d", "keep this one", "z", "new"],
    );
    assert_eq!(
        ops,
        [
            AlignOp::Replace(0, 0),
            AlignOp::Equal(1, 1),
            AlignOp::Equal(2, 2),
            AlignOp::Insert(3)
        ]
    );
    let unrelated = align(&["alpha"], &["omega"]);
    assert_eq!(unrelated, [AlignOp::Delete(0), AlignOp::Insert(0)]);
}
