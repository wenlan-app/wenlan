// SPDX-License-Identifier: Apache-2.0
use super::*;
use std::io::Write;

fn write(path: &Path, lines: &[serde_json::Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut file = std::fs::File::create(path).unwrap();
    for line in lines {
        writeln!(file, "{line}").unwrap();
    }
}

fn claude_user(text: &str, at: &str) -> serde_json::Value {
    serde_json::json!({"type": "user", "sessionId": "s1", "cwd": "/w/wenlan", "gitBranch": "main",
        "timestamp": at, "message": {"role": "user", "content": text}})
}

fn claude_ai(text: &str, at: &str) -> serde_json::Value {
    serde_json::json!({"type": "assistant", "sessionId": "s1", "timestamp": at,
        "message": {"role": "assistant", "content": [{"type": "text", "text": text}]}})
}

fn conv(said: &[&str]) -> FoundConversation {
    FoundConversation {
        tool: "claude-code".into(),
        external_id: said.first().copied().unwrap_or("x").into(),
        path: PathBuf::from("/t.jsonl"),
        len: 1,
        cwd: Some("/w/wenlan".into()),
        branch: None,
        said: said.iter().map(|s| s.to_string()).collect(),
        last_reply: String::new(),
        first_at: 0,
        last_at: 0,
        automated: false,
        on_behalf: false,
    }
}

#[test]
fn ux_thing_1_a_claude_code_conversation_keeps_what_was_typed_and_its_own_last_time() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects/-w-wenlan/s1.jsonl");
    write(
        &path,
        &[
            claude_user(
                "<command-name>/clear</command-name>",
                "2026-11-01T10:00:00Z",
            ),
            claude_user("Make home show what I'm working on", "2026-11-01T10:00:05Z"),
            claude_ai("Looking at the home page.", "2026-11-01T10:00:09Z"),
            serde_json::json!({"type": "user", "isMeta": true, "timestamp": "2026-11-01T10:01:00Z",
                "message": {"content": "Caveat: injected"}}),
            claude_user("Also hide finished ones", "2026-11-01T11:00:00Z"),
            claude_ai("Done: a card can be hidden.", "2026-11-01T11:05:00Z"),
            serde_json::json!({"type": "ai-title", "aiTitle": "Home"}),
        ],
    );
    let found = read_claude_code(&path).unwrap();
    assert_eq!(found.tool, "claude-code");
    assert_eq!(found.external_id, "s1");
    assert_eq!(found.cwd.as_deref(), Some("/w/wenlan"));
    assert_eq!(found.branch.as_deref(), Some("main"));
    assert_eq!(
        found.said,
        vec![
            "Make home show what I'm working on",
            "Also hide finished ones"
        ]
    );
    assert_eq!(found.last_reply, "Done: a card can be hidden.");
    assert_eq!(
        found.first_at,
        unix_seconds("2026-11-01T10:00:00Z").unwrap()
    );
    assert_eq!(found.last_at, unix_seconds("2026-11-01T11:05:00Z").unwrap());
    assert!(!found.automated && !found.on_behalf);
}

#[test]
fn ux_thing_1_a_conversation_started_by_a_program_or_on_the_persons_behalf_is_marked() {
    let dir = tempfile::tempdir().unwrap();
    let sdk = dir.path().join("projects/-w/sdk.jsonl");
    write(
        &sdk,
        &[
            serde_json::json!({"type": "user", "sessionId": "sdk", "entrypoint": "sdk-cli",
        "timestamp": "2026-11-01T10:00:00Z", "message": {"content": "run the eval"}}),
        ],
    );
    assert!(read_claude_code(&sdk).unwrap().automated);

    let rc = dir.path().join("projects/-w/rc.jsonl");
    write(
        &rc,
        &[
            serde_json::json!({"type": "user", "sessionId": "rc", "projectsUserTurn": {"id": 1},
        "timestamp": "2026-11-01T10:00:00Z",
        "message": {"content": "<task>Rebuild the release notes</task>"}}),
        ],
    );
    let found = read_claude_code(&rc).unwrap();
    assert!(found.on_behalf);
    assert_eq!(found.said, vec!["Rebuild the release notes"]);
}

#[test]
fn ux_thing_1_a_codex_conversation_reads_its_meta_and_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join("sessions/2026/11/01/rollout-2026-11-01-abc.jsonl");
    write(
        &path,
        &[
            serde_json::json!({"type": "session_meta", "timestamp": "2026-11-01T09:00:00Z",
                "payload": {"id": "abc", "cwd": "/w/site", "originator": "codex_cli_rs", "git": {"branch": "seo"}}}),
            serde_json::json!({"type": "event_msg", "timestamp": "2026-11-01T09:00:01Z",
                "payload": {"type": "user_message", "message": "Fix the sitemap"}}),
            serde_json::json!({"type": "response_item", "timestamp": "2026-11-01T09:02:00Z",
                "payload": {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "Fixed."}]}}),
        ],
    );
    let found = read_codex(&path).unwrap();
    assert_eq!(
        (found.tool.as_str(), found.external_id.as_str()),
        ("codex", "abc")
    );
    assert_eq!(found.cwd.as_deref(), Some("/w/site"));
    assert_eq!(found.branch.as_deref(), Some("seo"));
    assert_eq!(found.said, vec!["Fix the sitemap"]);
    assert_eq!(found.last_reply, "Fixed.");
    assert!(!found.automated);

    // Newer Codex records the person's turns only as messages; a later
    // session_meta (a sub-agent it started) does not change whose file it is.
    let newer = dir.path().join("sessions/2026/11/02/rollout-new.jsonl");
    write(
        &newer,
        &[
            serde_json::json!({"type": "session_meta", "timestamp": "2026-11-02T09:00:00Z",
                "payload": {"id": "new", "cwd": "/w/site", "originator": "Codex Desktop", "source": "vscode"}}),
            serde_json::json!({"type": "response_item", "timestamp": "2026-11-02T09:00:01Z",
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "<environment_context>cwd</environment_context>"}]}}),
            serde_json::json!({"type": "response_item", "timestamp": "2026-11-02T09:00:02Z",
                "payload": {"type": "message", "role": "user", "content": [{"type": "input_text", "text": "Rewrite the pricing page"}]}}),
            serde_json::json!({"type": "session_meta", "timestamp": "2026-11-02T09:01:00Z",
                "payload": {"id": "child", "source": {"subagent": {"name": "x"}}}}),
            serde_json::json!({"type": "event_msg", "timestamp": "2026-11-02T09:02:00Z",
                "payload": {"type": "user_message", "message": "Rewrite the pricing page"}}),
        ],
    );
    let found = read_codex(&newer).unwrap();
    assert_eq!(found.external_id, "new");
    assert_eq!(found.said, vec!["Rewrite the pricing page"]);
    assert!(!found.automated);

    let sub = dir.path().join("sessions/2026/11/02/rollout-sub.jsonl");
    write(
        &sub,
        &[
            serde_json::json!({"type": "session_meta", "timestamp": "2026-11-02T09:00:00Z",
        "payload": {"id": "sub", "originator": "Codex Desktop", "source": {"subagent": {"parent": "new"}}}}),
        ],
    );
    assert!(read_codex(&sub).unwrap().automated);

    let exec = dir.path().join("sessions/2026/11/01/rollout-exec.jsonl");
    write(
        &exec,
        &[
            serde_json::json!({"type": "session_meta", "timestamp": "2026-11-01T09:00:00Z",
        "payload": {"id": "ex", "originator": "codex_exec"}}),
        ],
    );
    assert!(read_codex(&exec).unwrap().automated);
}

#[test]
fn ux_home_5_scan_finds_only_conversations_whose_last_line_is_in_the_window() {
    let dir = tempfile::tempdir().unwrap();
    let recent = dir.path().join(".claude/projects/-w/new.jsonl");
    write(
        &recent,
        &[
            claude_user("new work", "2026-11-02T10:00:00Z"),
            claude_user("more", "2026-11-02T10:01:00Z"),
        ],
    );
    // Touched today, but the conversation itself ended a month ago.
    let old = dir.path().join(".claude/projects/-w/old.jsonl");
    write(&old, &[claude_user("old work", "2026-10-01T10:00:00Z")]);
    let roots = TranscriptRoots {
        claude_dir: Some(dir.path().join(".claude")),
        codex_dir: Some(dir.path().join(".codex")),
    };
    let now = unix_seconds("2026-11-03T00:00:00Z").unwrap();
    let found = scan(&roots, now, INTAKE_WINDOW_SECS);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].said[0], "new work");
}

#[test]
fn ux_thing_1_rules_drop_programs_single_questions_and_pasted_fixed_prompts() {
    let template = "You are reviewing this repository for release blockers today";
    let mut automated = conv(&["a", "b"]);
    automated.automated = true;
    let mut on_behalf = conv(&["Ship the release notes"]);
    on_behalf.on_behalf = true;
    let found = vec![
        automated,
        conv(&["what time is it in Tokyo"]),
        conv(&[]),
        conv(&[template, "ok"]),
        conv(&[template]),
        conv(&[
            &format!("{template}\nfocus on the storage migration please"),
            "and the tests",
        ]),
        on_behalf,
        conv(&["Plan the home redesign", "make cards smaller"]),
    ];
    let (kept, dropped) = keep_work(found);
    assert_eq!(
        dropped,
        Dropped {
            automated: 1,
            nothing_typed: 1,
            one_message: 1,
            fixed_prompt: 2
        }
    );
    let kept: Vec<_> = kept.iter().map(|c| c.said.clone()).collect();
    assert_eq!(
        kept,
        vec![
            vec![
                "focus on the storage migration please".to_string(),
                "and the tests".into()
            ],
            vec!["Ship the release notes".to_string()],
            vec![
                "Plan the home redesign".to_string(),
                "make cards smaller".into()
            ],
        ]
    );
}

#[test]
fn ux_thing_1_a_label_keeps_only_the_persons_spaces_and_needs_a_title_to_be_work() {
    let spaces = vec!["wenlan".to_string(), "writing".to_string()];
    let label = parse_label(
        "<think>hmm</think>{\"work\": true, \"title\": \"首頁改版\", \"space\": \"wenlan\", \"about\": \"a\", \"status\": \"b\"}",
        &spaces,
    )
    .unwrap();
    assert_eq!(
        label,
        Label {
            work: true,
            title: "首頁改版".into(),
            space: "wenlan".into(),
            about: "a".into(),
            status: "b".into()
        }
    );
    let label = parse_label(
        "{\"work\": true, \"title\": \"\", \"space\": \"elsewhere\"}",
        &spaces,
    )
    .unwrap();
    assert!(!label.work);
    assert_eq!(label.space, "");
    assert!(parse_label("no json here", &spaces).is_none());
}

#[test]
fn ux_thing_1_grouping_joins_existing_things_and_gives_every_conversation_a_thing() {
    let pending: Vec<Pending> = (0..6)
        .map(|i| Pending {
            session_id: format!("s{i}"),
            title: format!("t{i}"),
            about: String::new(),
            space: String::new(),
        })
        .collect();
    let answer = r#"{"join": [{"conversation": "C0", "piece": "T1"}, {"conversation": "C0", "piece": "T0"},
        {"conversation": "C9", "piece": "T0"}],
        "new": [{"title": "Home", "conversations": ["C1", "C2", "C0"]}],
        "leave_out": ["C4", "C1", "C4", "C8"]}"#;
    let grouping = parse_grouping(answer, &pending, 2).unwrap();
    assert_eq!(grouping.joins, vec![(0, 1)]);
    // A chore is left out once; one already in a piece stays there.
    assert_eq!(grouping.left_out, vec![4]);
    // C3 and C5 the model forgot stand alone under their own titles.
    assert_eq!(
        grouping.new,
        vec![
            ("Home".into(), vec![1, 2]),
            ("t3".into(), vec![3]),
            ("t5".into(), vec![5])
        ]
    );
}

#[test]
fn ux_thing_1_one_new_group_swallowing_a_batch_is_broken_up() {
    let pending: Vec<Pending> = (0..20)
        .map(|i| Pending {
            session_id: format!("s{i}"),
            title: format!("t{i}"),
            about: String::new(),
            space: String::new(),
        })
        .collect();
    let all: Vec<String> = (0..20).map(|i| format!("\"C{i}\"")).collect();
    let answer = format!(
        "{{\"new\": [{{\"title\": \"Everything\", \"conversations\": [{}]}}]}}",
        all.join(",")
    );
    let grouping = parse_grouping(&answer, &pending, 0).unwrap();
    assert_eq!(grouping.new.len(), 20);
    assert!(grouping.new.iter().all(|(_, members)| members.len() == 1));
}

#[test]
fn ux_thing_1_titles_follow_the_app_language() {
    assert_eq!(language_name(Some("zh-Hant")), "Traditional Chinese");
    assert_eq!(language_name(Some("zh-Hans")), "Simplified Chinese");
    assert_eq!(language_name(Some("en-US")), "English");
    assert_eq!(language_name(None), "English");
    let prompt = label_prompt(
        &conv(&["a", "b"]),
        &["wenlan".into()],
        "Traditional Chinese",
    );
    assert!(prompt.contains("in Traditional Chinese"));
    assert!(prompt.contains("folder \"wenlan\""));
}

#[test]
fn ux_thing_1_a_worktree_belongs_to_its_main_checkout() {
    let dir = tempfile::tempdir().unwrap();
    let main = dir.path().join("repo");
    std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    let wt = dir.path().join("wt");
    std::fs::create_dir_all(wt.join("src")).unwrap();
    std::fs::write(
        wt.join(".git"),
        format!("gitdir: {}/.git/worktrees/wt\n", main.display()),
    )
    .unwrap();
    assert_eq!(
        repo_root_of(&wt.join("src").to_string_lossy()),
        Some(main.to_string_lossy().into_owned())
    );
    assert_eq!(
        repo_root_of(&main.to_string_lossy()),
        Some(main.to_string_lossy().into_owned())
    );
    assert_eq!(repo_root_of(&dir.path().to_string_lossy()), None);
}
