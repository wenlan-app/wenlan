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
        touched: Vec::new(),
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
fn ux_thing_1_a_claude_code_conversation_keeps_the_files_and_rules_it_worked_on() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("projects/-w-wenlan/s1.jsonl");
    let tool = |name: &str, input: serde_json::Value| {
        serde_json::json!({"type": "assistant", "sessionId": "s1", "timestamp": "2026-11-01T10:01:00Z",
            "message": {"role": "assistant", "content": [{"type": "tool_use", "name": name, "input": input}]}})
    };
    write(
        &path,
        &[
            claude_user("Make home cards calmer", "2026-11-01T10:00:00Z"),
            tool(
                "Edit",
                serde_json::json!({"file_path": "/w/wenlan/src/home.tsx"}),
            ),
            tool(
                "Edit",
                serde_json::json!({"file_path": "/w/wenlan/src/home.tsx"}),
            ),
            tool(
                "Write",
                serde_json::json!({"file_path": "/w/wenlan/docs/ux/spec.md",
                "content": "a spec rule id is only mentioned in written text"}),
            ),
            // Outside the conversation's folder: not this work's file.
            tool(
                "Write",
                serde_json::json!({"file_path": "/tmp/scratch.txt"}),
            ),
            tool(
                "Read",
                serde_json::json!({"file_path": "/w/wenlan/src/read-only.tsx"}),
            ),
            tool(
                "Bash",
                serde_json::json!({"command": "cargo test ux_home_4 # UX-HOME-4"}),
            ),
            claude_ai("Done for UX-HOME-4.", "2026-11-01T10:05:00Z"),
        ],
    );
    let found = read_claude_code(&path).unwrap();
    assert_eq!(
        found.touched,
        vec!["src/home.tsx", "docs/ux/spec.md", "UX-HOME-4"]
    );
}

#[test]
fn ux_thing_1_a_codex_conversation_keeps_the_files_its_patches_changed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions/2026/11/01/rollout-p.jsonl");
    write(
        &path,
        &[
            serde_json::json!({"type": "session_meta", "timestamp": "2026-11-01T09:00:00Z",
                "payload": {"id": "p", "cwd": "/w/site", "originator": "codex_cli_rs"}}),
            serde_json::json!({"type": "event_msg", "timestamp": "2026-11-01T09:00:01Z",
                "payload": {"type": "user_message", "message": "Fix the sitemap"}}),
            serde_json::json!({"type": "response_item", "timestamp": "2026-11-01T09:01:00Z",
                "payload": {"type": "custom_tool_call", "name": "apply_patch",
                    "input": "*** Begin Patch\n*** Update File: /w/site/app/sitemap.ts\n@@\n-a\n+b\n*** End Patch"}}),
            // A patch written inside a script string, line breaks escaped.
            serde_json::json!({"type": "response_item", "timestamp": "2026-11-01T09:02:00Z",
                "payload": {"type": "custom_tool_call", "name": "exec",
                    "input": "tools.apply_patch(\"*** Begin Patch\\n*** Add File: app/robots.ts\\n+x\\n*** End Patch\")"}}),
            serde_json::json!({"type": "response_item", "timestamp": "2026-11-01T09:03:00Z",
                "payload": {"type": "function_call", "name": "shell",
                    "arguments": "{\"command\": [\"rg\", \"SEO-MAP-2\"]}"}}),
        ],
    );
    let found = read_codex(&path).unwrap();
    assert_eq!(
        found.touched,
        vec!["app/robots.ts", "app/sitemap.ts", "SEO-MAP-2"]
    );
}

#[test]
fn ux_thing_1_grouping_sees_what_each_conversation_worked_on() {
    let pending = vec![Pending {
        session_id: "s0".into(),
        title: "Calmer cards".into(),
        about: String::new(),
        space: String::new(),
        status: String::new(),
        touched: vec!["src/home.tsx".into(), "UX-HOME-4".into()],
    }];
    let open = vec![OpenThing {
        id: "t".into(),
        title: "Redesign".into(),
        about: String::new(),
        space: String::new(),
        news: Vec::new(),
        touched: vec!["src/home.tsx".into()],
    }];
    let prompt = group_prompt(&pending, &open, &[], "English");
    assert!(prompt.contains("C0. Calmer cards []:  | worked on: src/home.tsx, UX-HOME-4 | now:"));
    assert!(prompt.contains("T0. Redesign []:  | worked on: src/home.tsx"));
    assert!(prompt.contains("not by how alike the titles are"));
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
            status: String::new(),
            touched: Vec::new(),
        })
        .collect();
    let answer = r#"{"join": [{"conversation": "C0", "piece": "T1"}, {"conversation": "C0", "piece": "T0"},
        {"conversation": "C9", "piece": "T0"}],
        "new": [{"title": "Home", "conversations": ["C1", "C2", "C0"], "now": "Two cards left"}],
        "leave_out": ["C4", "C1", "C4", "C8"],
        "now": [{"piece": "T1", "line": "Old line"}, {"piece": "T1", "line": "Cards done; spacing next"},
                {"piece": "T7", "line": "No such piece"}, {"piece": "T0", "line": "  "}]}"#;
    let grouping = parse_grouping(answer, &pending, 2).unwrap();
    assert_eq!(grouping.joins, vec![(0, 1)]);
    // A chore is left out once; one already in a piece stays there.
    assert_eq!(grouping.left_out, vec![4]);
    // Where each piece stands: the last line for a piece wins, empty ones go.
    assert_eq!(
        grouping.lines,
        vec![
            (Piece::New(0), "Two cards left".to_string()),
            (Piece::Open(1), "Cards done; spacing next".to_string())
        ]
    );
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
            status: String::new(),
            touched: Vec::new(),
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
    // Real use (2026-11-04): a design discussion with no code came out as
    // not work.
    assert!(prompt.contains("even when nothing was built"));
}

/// Real use (2026-11-06): home cards read "PR #1168 night points… (ed2f2dcf)".
#[test]
fn ux_home_6_titles_and_lines_are_asked_for_in_plain_words() {
    let label = label_prompt(&conv(&["a", "b"]), &[], "English");
    let group = group_prompt(&[], &[], &[], "English");
    for prompt in [label, group] {
        assert!(prompt.contains("no pull request or issue numbers, commit hashes"));
    }
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

#[test]
fn ux_thing_3_the_grouping_names_look_alike_things_only_as_pairs_to_ask_about() {
    let answer = r#"{"same": [["T2", "T0"], ["T0", "T2"], ["T1", "T1"], ["T0", "T9"], ["T1"]]}"#;
    let grouping = parse_grouping(answer, &[], 3).unwrap();
    assert_eq!(grouping.same, vec![(0, 2)]);
    assert!(grouping.joins.is_empty() && grouping.new.is_empty());
    assert!(
        group_prompt(&[], &[], &[], "English").contains("they are not merged, the person is asked")
    );
}

/// UX-CHAT-5: a night's reading keeps only points with text from a real
/// conversation; a point replaces a kept one at most once.
#[test]
fn ux_chat_5_a_nights_points_come_from_real_conversations() {
    let answer = r#"thinking...
{"points": [
  {"text": "  Tagline is You do the thinking.  ", "settled": true, "from": "C0", "replaces": "P1"},
  {"text": "Tagline is You do the thinking.", "settled": true, "from": "C0"},
  {"text": "Pricing page is open.", "settled": false, "from": "C1", "replaces": "P1"},
  {"text": "No source.", "settled": true, "from": "C7"},
  {"text": "", "settled": true, "from": "C0"},
  {"text": "Bad replace.", "settled": true, "from": "C0", "replaces": "P9"}
]}"#;
    let points = parse_dream(answer, 2, 2, 0, 0).unwrap();
    assert_eq!(
        points,
        vec![
            DreamPoint {
                text: "Tagline is You do the thinking.".into(),
                decided: true,
                from: 0,
                replaces: Some(1),
                map: None,
                topic: None,
            },
            DreamPoint {
                text: "Pricing page is open.".into(),
                decided: false,
                from: 1,
                replaces: None,
                map: None,
                topic: None,
            },
            DreamPoint {
                text: "Bad replace.".into(),
                decided: true,
                from: 0,
                replaces: None,
                map: None,
                topic: None,
            },
        ]
    );
    assert_eq!(parse_dream(r#"{"points": []}"#, 1, 0, 0, 0), Some(vec![]));
    assert!(parse_dream("no json", 1, 0, 0, 0).is_none());
    let prompt = dream_prompt(
        "Site",
        &[],
        &[PriorPoint {
            id: "m".into(),
            text: "Kept.".into(),
            decided: false,
        }],
        &[MapPoint {
            text: "Which tagline?".into(),
            decided: None,
        }],
        &[],
        "English",
    );
    assert!(prompt.contains("P0 (open): Kept.") && prompt.contains("in English"));
    assert!(prompt.contains("N0: Which tagline?"), "{prompt}");
    // Real use (2026-11-04): merge approvals, handoffs and long sentences
    // made noisy points, and a question answered later stayed open.
    assert!(prompt.contains("approving a merge, a review, a handoff or a run"));
    assert!(prompt.contains("answered or done later in the same conversations is settled"));
    assert!(prompt.contains("at most 15 words"));
    assert!(prompt.contains("where a pull request stands are not decisions"));
    // No topics to offer: the question is not asked.
    assert!(
        !prompt.contains("\"topic\"") && !prompt.contains("big map"),
        "{prompt}"
    );
}

/// UX-CHAT-5: a point can say which point on the thing's map it is, once
/// per map point, so the map and long-term memory stay one point.
#[test]
fn ux_chat_5_a_nights_point_names_its_map_point() {
    let answer = r#"{"points": [
  {"text": "Tagline is You do the thinking.", "settled": true, "from": "C0", "map": "N1"},
  {"text": "Pricing waits.", "settled": false, "from": "C0", "map": "N1"},
  {"text": "Docs move.", "settled": true, "from": "C0", "map": "N5"}
]}"#;
    let points = parse_dream(answer, 1, 0, 2, 0).unwrap();
    assert_eq!(
        points.iter().map(|p| p.map).collect::<Vec<_>>(),
        [Some(1), None, None]
    );
}

/// UX-CHAT-5: in the one night call, each point names its topic from the list,
/// or null. A made-up id leaves the point without a topic, never without the point.
#[test]
fn ux_chat_5_a_nights_point_names_its_topic_from_the_list() {
    let prompt = dream_prompt(
        "Site",
        &[],
        &[],
        &[],
        &["GitHub".into(), "Pricing".into()],
        "English",
    );
    assert!(prompt.contains("T0: GitHub\nT1: Pricing"), "{prompt}");
    assert!(prompt.contains("do not make up a topic"));
    assert!(prompt.contains("\"topic\": \"T0\" or null"));
    let answer = r#"{"points": [
  {"text": "Keep the repository private.", "settled": true, "from": "C0", "topic": "T0"},
  {"text": "Which plan first?", "settled": false, "from": "C0", "topic": null},
  {"text": "Docs move.", "settled": true, "from": "C0", "topic": "T9"},
  {"text": "Made up a name.", "settled": true, "from": "C0", "topic": "Billing"},
  {"text": "Never said.", "settled": true, "from": "C0"}
]}"#;
    let points = parse_dream(answer, 1, 0, 0, 2).unwrap();
    assert_eq!(
        points.iter().map(|p| p.topic).collect::<Vec<_>>(),
        [Some(0), None, None, None, None]
    );
}

/// UX-CHAT-5: a name is offered because the person typed it only when it is
/// specific. Product names stay; everyday words and names of code do not.
#[test]
fn ux_chat_5_only_specific_names_put_a_topic_on_the_list() {
    let raw = "push it to GitHub, then the memory and Memory work; memory again. \
Long-term memory. 夜間整理 is on macOS. ChatGPT too.";
    let lower = raw.to_lowercase();
    let hits = |name: &str| specific_name_hits(name, raw, &lower);
    assert_eq!(hits("GitHub"), Some(1));
    assert_eq!(hits("macOS"), Some(1));
    assert_eq!(hits("ChatGPT"), Some(1));
    assert_eq!(hits("Long-Term Memory"), Some(1), "several words");
    assert_eq!(hits("Memory"), None, "mostly written without capitals");
    assert_eq!(hits("memory"), None, "an everyday word");
    assert_eq!(hits("work"), None);
    assert_eq!(hits("Mac"), None, "too short to mean anything");
    assert_eq!(hits("夜間整理"), Some(1));
    assert_eq!(hits("夜間"), None);

    for name in [
        "GitHub",
        "macOS",
        "ChatGPT",
        "Claude Code",
        "夜間整理",
        "LLM",
    ] {
        assert!(topic_name_offered(name), "{name}");
    }
    for name in [
        "EXIT CODE",
        "dream_with",
        "src/lib",
        "main.rs",
        "foo()",
        "#1150",
        "unit test",
        "--flag",
        "https://x.y",
        "  ",
    ] {
        assert!(!topic_name_offered(name), "{name}");
    }
}

/// UX-CHAT-5: connecting old points asks only for topics, answered by id.
#[test]
fn ux_chat_5_old_points_are_connected_by_id_or_left_alone() {
    let prompt = night_links_prompt("Site", &["Keep it private.".into()], &["GitHub".into()]);
    assert!(
        prompt.contains("P0: Keep it private.") && prompt.contains("T0: GitHub"),
        "{prompt}"
    );
    let answer = r#"{"points": [
  {"point": "P0", "topic": "T0"},
  {"point": "P0", "topic": "T1"},
  {"point": "P1", "topic": null},
  {"point": "P2", "topic": "T9"},
  {"point": "P7", "topic": "T0"}
]}"#;
    assert_eq!(parse_night_links(answer, 3, 2), Some(vec![(0, 0)]));
    assert_eq!(parse_night_links("no json", 3, 2), None);
}

/// UX-VIEW-6: the conclusion says only what the points say. A sentence that
/// names no real point is dropped; at most two are kept, each cut short.
#[test]
fn ux_view_6_a_conclusion_sentence_needs_a_point_behind_it() {
    let answer = r#"thinking… {"conclusion": [
        {"text": "  The tagline is settled.  ", "points": ["P0", "P0", "P9"]},
        {"text": "Something nobody said.", "points": []},
        {"text": "A made-up point.", "points": ["P7"]},
        {"text": "Pricing is still open.", "points": ["P1"]},
        {"text": "A third sentence.", "points": ["P1"]}
    ]}"#;
    let parsed = parse_conclusion(answer, 2).unwrap();
    assert_eq!(
        parsed,
        [
            ConclusionSentence {
                text: "The tagline is settled.".into(),
                points: vec![0]
            },
            ConclusionSentence {
                text: "Pricing is still open.".into(),
                points: vec![1]
            },
        ]
    );
    assert!(parse_conclusion("not json", 2).is_none());
    assert_eq!(parse_conclusion(r#"{"conclusion": []}"#, 2), Some(vec![]));
    let long = format!(
        r#"{{"conclusion": [{{"text": "{}", "points": ["P0"]}}]}}"#,
        "a".repeat(400)
    );
    assert!(parse_conclusion(&long, 1).unwrap()[0].text.chars().count() <= MAX_CONCLUSION_CHARS);
}

/// UX-VIEW-6: the conclusion is written from the newest standing points, in
/// the app's language, marked settled or open.
#[test]
fn ux_view_6_the_conclusion_prompt_lists_the_newest_points() {
    let points: Vec<PriorPoint> = (0..MAX_CONCLUSION_POINTS + 5)
        .map(|i| PriorPoint {
            id: format!("mem_{i}"),
            text: format!("point {i}"),
            decided: i % 2 == 0,
        })
        .collect();
    let kept = conclusion_points(&points);
    assert_eq!(kept.len(), MAX_CONCLUSION_POINTS);
    assert_eq!(kept[0].id, "mem_5");
    let prompt = conclusion_prompt("Site redesign", kept, "Traditional Chinese");
    assert!(prompt.contains("\"Site redesign\""));
    assert!(prompt.contains("P0 (open): point 5"));
    assert!(prompt.contains("in Traditional Chinese"));
    assert!(!prompt.contains("point 4\n"));
}
