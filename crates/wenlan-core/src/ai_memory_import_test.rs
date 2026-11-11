// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::fs;

fn roots(dir: &Path) -> TranscriptRoots {
    TranscriptRoots {
        claude_dir: Some(dir.join(".claude")),
        codex_dir: Some(dir.join(".codex")),
    }
}

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
}

#[test]
fn scan_reads_claude_code_topic_files_and_index_facts_and_codex_items() {
    let tmp = tempfile::tempdir().unwrap();
    let project = tmp.path().join(".claude/projects/-Users-me-Repos-wenlan");
    write(
        &project.join("memory/MEMORY.md"),
        "# Memory\n\n- [Storage](storage.md) — database is the source of truth\n\
         - The page check can leave a page quietly out of date after it fails.\n",
    );
    write(
        &project.join("memory/storage.md"),
        "---\nname: storage\ndescription: The database is the source of truth\nmetadata:\n  type: project\n---\n\nMarkdown files are a projection written from the database.\n",
    );
    // A transcript next to the memory folder is not memory.
    write(&project.join("abc.jsonl"), "{}\n");
    write(
        &tmp.path().join(".codex/memories/MEMORY.md"),
        "## Lessons\n\n- Borrowing outside code needs a license check first.\n\
         - Renaming a repository can break npm provenance\n  until the package is published again.\n\nok\n",
    );

    let found = scan(&roots(tmp.path()));
    let texts: Vec<_> = found.iter().map(|m| (m.tool, m.text.as_str())).collect();
    assert_eq!(
        texts,
        vec![
            (
                MemoryTool::ClaudeCode,
                "The page check can leave a page quietly out of date after it fails."
            ),
            (
                MemoryTool::ClaudeCode,
                "The database is the source of truth\nMarkdown files are a projection written from the database."
            ),
            (
                MemoryTool::Codex,
                "Borrowing outside code needs a license check first."
            ),
            (
                MemoryTool::Codex,
                "Renaming a repository can break npm provenance\nuntil the package is published again."
            ),
        ]
    );
    assert_eq!(found[0].project_dir.as_deref(), Some(project.as_path()));
    assert_eq!(found[2].project_dir, None);
}

#[test]
fn scan_reads_an_exact_repeat_in_one_project_once() {
    let tmp = tempfile::tempdir().unwrap();
    let memory = tmp.path().join(".claude/projects/p/memory");
    write(
        &memory.join("a.md"),
        "Releases go out only through the queue.\n",
    );
    write(
        &memory.join("b.md"),
        "releases go out only through the queue\n",
    );
    assert_eq!(scan(&roots(tmp.path())).len(), 1);
}

#[test]
fn scan_with_no_memory_folders_finds_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(scan(&roots(tmp.path())).is_empty());
    assert!(scan(&TranscriptRoots {
        claude_dir: None,
        codex_dir: None
    })
    .is_empty());
}

#[test]
fn sensitive_memories_are_caught() {
    for text in [
        "Deploy key is sk-ant-api03-abcdefghijklmnopqrstuvwxyz0123",
        "Use token ghp_abcdefghijklmnopqrstuvwxyz0123456789 for CI",
        "password: hunter2hunter2",
        "Reach Dana at dana@example.com about the launch",
        "Authorization: Bearer abcdefghijklmnopqrstuvwxyz012345",
    ] {
        assert!(looks_sensitive(text), "{text}");
    }
    for text in [
        "The database is the source of truth; Markdown is a projection.",
        "Version 0.21.4 shipped the new home page.",
        "Read session 3f2a9c1e-1b2c-4d5e-8f90-1234567890ab before changing it.",
    ] {
        assert!(!looks_sensitive(text), "{text}");
    }
}

#[test]
fn keep_needs_useful_and_three_noes_and_files_the_persons_own_life_apart() {
    let raw = r#"Here you go:
```json
{"answers": [
 {"m": "M0", "useful": true, "tool": false, "expires": false, "others": false, "personal": false},
 {"m": "M1", "useful": true, "tool": true, "expires": false, "others": false, "personal": false},
 {"m": "M2", "useful": true, "tool": false, "expires": true, "others": false, "personal": false},
 {"m": "M3", "useful": true, "tool": false, "expires": false, "personal": false},
 {"m": "M4", "useful": true, "tool": false, "expires": false, "others": false, "personal": true},
 {"m": "M5", "useful": true, "tool": false, "expires": false, "others": true, "personal": true},
 {"m": "M6", "useful": true, "tool": false, "expires": false, "others": false},
 {"m": "M9", "useful": true, "tool": false, "expires": false, "others": false, "personal": false}
]}
```"#;
    assert_eq!(
        parse_keep(raw, 7),
        vec![
            Keep::Work,
            Keep::Drop,
            Keep::Drop,
            // Not answered: unsure.
            Keep::Drop,
            Keep::Personal,
            // Someone else's personal details.
            Keep::Drop,
            // Unsure where it belongs.
            Keep::Drop,
        ]
    );
}

#[test]
fn keep_trusts_neither_of_two_answers_for_one_memory() {
    let raw = r#"{"answers": [
 {"m": "M0", "useful": true, "tool": false, "expires": false, "others": false, "personal": false},
 {"m": "M0", "useful": true, "tool": false, "expires": false, "others": false, "personal": false}]}"#;
    assert_eq!(parse_keep(raw, 1), vec![Keep::Drop]);
    assert_eq!(parse_keep("not json", 2), vec![Keep::Drop, Keep::Drop]);
}

#[test]
fn keep_prompt_lists_every_memory_on_one_line() {
    let prompt = keep_prompt(&["first\nline", "second"]);
    assert!(prompt.contains("M0: first line\n"));
    assert!(prompt.contains("M1: second"));
    assert!(prompt.contains("\"useful\": false"));
}

fn page(id: &str, text: &str) -> Candidate {
    Candidate {
        kind: CandidateKind::Page,
        id: id.into(),
        title: id.into(),
        text: text.into(),
    }
}

#[test]
fn match_takes_same_only_with_a_candidate_shown_next_to_that_memory() {
    let a = [page("storage", "The database is the source of truth.")];
    let b = [page("release", "Releases go through the queue.")];
    let c: [Candidate; 0] = [];
    let items = [
        MatchItem {
            memory: "DB is canonical",
            candidates: &a,
        },
        MatchItem {
            memory: "Ship via queue",
            candidates: &b,
        },
        MatchItem {
            memory: "Lore needs a license check",
            candidates: &c,
        },
        MatchItem {
            memory: "Other",
            candidates: &b,
        },
    ];
    let raw = r#"{"answers": [
 {"m": "M0", "verdict": "same", "with": "M0K0"},
 {"m": "M1", "verdict": "same", "with": "M1K3"},
 {"m": "M2", "verdict": "new", "with": null},
 {"m": "M3", "verdict": "same", "with": "M1K0"}
]}"#;
    assert_eq!(
        parse_match(raw, &items),
        vec![
            Verdict::Same { with: 0 },
            // No such candidate.
            Verdict::Conflict,
            Verdict::New,
            // Named another memory's candidate.
            Verdict::Conflict,
        ]
    );
}

#[test]
fn match_left_out_or_unparsable_is_a_conflict() {
    let c = [page("p", "text")];
    let items = [
        MatchItem {
            memory: "x",
            candidates: &c,
        },
        MatchItem {
            memory: "y",
            candidates: &c,
        },
    ];
    let raw = r#"{"answers": [{"m": "M1", "verdict": "new"}]}"#;
    assert_eq!(
        parse_match(raw, &items),
        vec![Verdict::Conflict, Verdict::New]
    );
    assert_eq!(
        parse_match("sorry", &items),
        vec![Verdict::Conflict, Verdict::Conflict]
    );
}

#[test]
fn match_prompt_shows_candidates_under_their_memory() {
    let c = [page("Storage", "The database is the source of truth.")];
    let prompt = match_prompt(&[MatchItem {
        memory: "DB is canonical",
        candidates: &c,
    }]);
    assert!(prompt.contains("M0: DB is canonical\n  M0K0 (page) Storage: The database"));
    assert!(!prompt.contains("replaces"));
}
