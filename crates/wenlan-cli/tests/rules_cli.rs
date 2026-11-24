// SPDX-License-Identifier: Apache-2.0
//! Terminal entry points for saving Space rules and managing personal setup.

mod common;

use assert_cmd::Command;
use common::spawn_stub;
use predicates::prelude::*;
use serde_json::json;
use std::{fs, path::Path};
use wenlan_types::rulesync::{AdoptionPlan, AdoptionSkillOutcome};

fn cli(base: &str) -> Command {
    let mut cmd = Command::cargo_bin("wenlan").unwrap();
    cmd.env("WENLAN_HOST", base)
        .env("WENLAN_NO_AUTOSTART", "1")
        .env_remove("WENLAN_AGENT_NAME")
        .env_remove("WENLAN_SPACE")
        .env_remove("WENLAN_DEFAULT_SPACE");
    cmd
}

fn personal_rules_cli(state: &Path) -> Command {
    let home = state.join("ai-tools/home");
    let mut cmd = cli("http://127.0.0.1:1");
    cmd.env("HOME", &home)
        .env("CODEX_HOME", home.join(".codex"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("WENLAN_DEV_STATE_DIR", state)
        .env("WENLAN_DATA_DIR", state.join("unused-data-root"))
        .env_remove("WENLAN_RULESYNC")
        .env_remove("WENLAN_SESSION")
        .env_remove("CODEX_THREAD_ID");
    cmd
}

#[test]
fn rules_switch_persists_and_environment_overrides_it() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    fs::create_dir_all(home.join(".gemini")).unwrap();
    fs::write(home.join(".gemini/GEMINI.md"), "Gemini rules.\n").unwrap();
    let marker = state.path().join("unused-data-root/rulesync.on");

    personal_rules_cli(state.path())
        .args(["rules", "on"])
        .assert()
        .success();
    assert!(marker.is_file());
    for (override_value, expected) in [(None, true), (Some("0"), false)] {
        let mut command = personal_rules_cli(state.path());
        if let Some(value) = override_value {
            command.env("WENLAN_RULESYNC", value);
        }
        let output = command
            .args(["rules", "adopt", "--dry-run", "--format", "json"])
            .assert()
            .success();
        let plan: AdoptionPlan = serde_json::from_slice(&output.get_output().stdout).unwrap();
        assert_eq!(
            plan.files
                .iter()
                .any(|file| file.path == ".gemini/GEMINI.md"),
            expected
        );
    }
    personal_rules_cli(state.path())
        .args(["rules", "off"])
        .assert()
        .success();
    assert!(!marker.exists());
}

#[cfg(unix)]
/// A setup run with a stub daemon, a fake home with one rules file and one
/// skill, and no AI tool on PATH. Returns the command, the plan and the home.
fn setup_fixture(
    state: &Path,
    status_adopted: bool,
    plan: &AdoptionPlan,
) -> (Command, std::sync::mpsc::Receiver<(String, String)>) {
    setup_fixture_with(state, status_adopted, plan, &[])
}

#[cfg(unix)]
/// `unmanaged` names the planned tools that the status route reports as not managed.
fn setup_fixture_with(
    state: &Path,
    status_adopted: bool,
    plan: &AdoptionPlan,
    unmanaged: &[&str],
) -> (Command, std::sync::mpsc::Receiver<(String, String)>) {
    fs::create_dir_all(state.join("unused-data-root")).unwrap();
    fs::write(state.join("unused-data-root/person.key"), "a".repeat(64)).unwrap();
    let tools: Vec<_> = plan
        .tools
        .iter()
        .map(|tool| {
            let state = if unmanaged.contains(&tool.name.as_str()) {
                "not_managed"
            } else {
                "up_to_date"
            };
            json!({"id":tool.name.to_lowercase(),"name":tool.name,"state":state,"counts":{"rules":tool.rule_files,"skills":tool.skills,"commands":0,"subagents":0,"mcp":0}})
        })
        .collect();
    let status = json!({"enabled":true,"adopted":status_adopted,"can_adopt":true,"can_restore":false,"in_sync":true,"proposals":[],"review_tools":{},"tools":tools});
    let report = json!({"written":[],"blocked":[],"proposals":[],"message":"done"});
    // Every request to the daemon first asks for health.
    let health = json!({"status":"ok","db_initialized":true,"version":"test"}).to_string();
    let mut routes = vec![("GET /api/health", health); 10];
    routes.extend([
        ("GET /api/rules/setup/status", status.to_string()),
        ("GET /api/rules/setup/status", status.to_string()),
        (
            "POST /api/rules/setup/adopt/preview",
            serde_json::to_string(plan).unwrap(),
        ),
        ("POST /api/rules/setup/adopt", report.to_string()),
        ("POST /api/rules/setup/sync", report.to_string()),
    ]);
    let (base, requests) = spawn_stub(routes);
    let empty_path = state.join("no-tools");
    fs::create_dir_all(&empty_path).unwrap();
    // The only AI tool on PATH, and it does nothing.
    let fake_tool = empty_path.join("claude");
    fs::write(&fake_tool, "#!/bin/sh\nexit 0\n").unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake_tool, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let mut cmd = personal_rules_cli(state);
    cmd.env("WENLAN_HOST", base)
        .env("PATH", empty_path)
        .current_dir(state)
        .args(["setup", "--finish"]);
    (cmd, requests)
}

#[cfg(unix)]
fn setup_home(state: &Path) -> AdoptionPlan {
    let home = state.join("ai-tools/home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(
        home.join(".codex/AGENTS.md"),
        "First fixture rule.\n\nSecond fixture rule.\nThird fixture rule.\n",
    )
    .unwrap();
    let skill = home.join(".codex/skills/example/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::write(
        &skill,
        "---\nname: example\ndescription: Fixture\n---\nBody.\n",
    )
    .unwrap();
    wenlan_core::rulesync_personal::plan_adoption(&home).unwrap()
}

#[cfg(unix)]
fn requested(requests: &std::sync::mpsc::Receiver<(String, String)>) -> Vec<String> {
    requests.try_iter().map(|(path, _)| path).collect()
}

#[cfg(unix)]
#[test]
fn ux_cli_8_setup_turns_sync_on_adopts_and_reports_the_counts() {
    let state = tempfile::tempdir().unwrap();
    let plan = setup_home(state.path());
    let (mut cmd, requests) = setup_fixture(state.path(), false, &plan);
    let output = cmd
        .args(["--format", "table"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("3 rules, 1 skill."), "{text}");
    assert!(!text.contains("follow their source"), "{text}");
    assert!(!text.contains(" copied)"), "{text}");
    assert!(text.contains("To undo: wenlan rules restore"), "{text}");
    assert!(!text.contains(state.path().to_str().unwrap()), "{text}");
    assert!(state.path().join("unused-data-root/rulesync.on").is_file());
    let seen = requested(&requests);
    for route in [
        "POST /api/rules/setup/adopt/preview",
        "POST /api/rules/setup/adopt",
        "POST /api/rules/setup/sync",
    ] {
        assert!(seen.iter().any(|seen| seen == route), "{route}: {seen:?}");
    }
    assert!(seen.iter().all(|path| !path.contains("/share")), "{seen:?}");
}

#[cfg(unix)]
#[test]
fn setup_report_names_only_tools_that_end_up_managed() {
    let state = tempfile::tempdir().unwrap();
    let mut plan = setup_home(state.path());
    plan.tools.push(wenlan_types::rulesync::AdoptionTool {
        name: "Antigravity".into(),
        rule_files: 1,
        skills: 0,
        other_files: 0,
    });
    let (mut cmd, _requests) = setup_fixture_with(state.path(), false, &plan, &["Antigravity"]);
    let output = cmd
        .args(["--format", "table"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let text = String::from_utf8(output).unwrap();
    assert!(text.contains("Codex"), "{text}");
    assert!(!text.contains("Antigravity"), "{text}");
}

#[cfg(unix)]
#[test]
fn setup_in_an_ai_session_adopts_only_the_plan_it_previewed() {
    let state = tempfile::tempdir().unwrap();
    let mut plan = setup_home(state.path());
    plan.plan_id = "0123456789ab".into();
    let (mut cmd, requests) = setup_fixture(state.path(), false, &plan);
    for_agent_session(cmd.args(["--format", "table"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("To undo: wenlan rules restore"));
    let adopt = requests
        .try_iter()
        .find(|(path, _)| path == "POST /api/rules/setup/adopt")
        .expect("setup adopts");
    let request: wenlan_types::rulesync::AdoptRequest = serde_json::from_str(&adopt.1).unwrap();
    assert_eq!(request.confirmed_plan_id.as_deref(), Some("0123456789ab"));
    assert!(requests
        .try_iter()
        .all(|(path, _)| !path.contains("/share")));
}

#[cfg(unix)]
#[test]
fn ux_cli_8_setup_noninteractive_yes_never_requests_share() {
    let state = tempfile::tempdir().unwrap();
    let plan = setup_home(state.path());
    let (mut cmd, requests) = setup_fixture(state.path(), false, &plan);
    cmd.arg("--yes").assert().success();
    let seen = requested(&requests);
    assert!(seen.iter().all(|path| !path.contains("/share")), "{seen:?}");
}

#[cfg(unix)]
#[test]
fn setup_json_carries_the_same_counts() {
    let state = tempfile::tempdir().unwrap();
    let plan = setup_home(state.path());
    let (mut cmd, _requests) = setup_fixture(state.path(), false, &plan);
    let output = cmd.args(["--format", "json"]).assert().success();
    let text = String::from_utf8(output.get_output().stdout.clone()).unwrap();
    let start = text.find("{\n  \"tools\"").expect(&text);
    let value: serde_json::Value = serde_json::Deserializer::from_str(&text[start..])
        .into_iter()
        .next()
        .unwrap()
        .unwrap();
    for field in [
        "tools",
        "rules",
        "skills",
        "following",
        "copied",
        "left_alone",
    ] {
        assert!(value.get(field).is_some(), "{field}: {value}");
    }
    assert_eq!(value["rules"], 3);
}

#[cfg(unix)]
#[test]
fn second_setup_does_not_adopt_again_or_touch_files() {
    let state = tempfile::tempdir().unwrap();
    let plan = setup_home(state.path());
    let rules = state.path().join("ai-tools/home/.codex/AGENTS.md");
    let before = (
        fs::read(&rules).unwrap(),
        fs::metadata(&rules).unwrap().modified().unwrap(),
    );
    let (mut cmd, requests) = setup_fixture(state.path(), true, &plan);
    let output = cmd.assert().success().get_output().stdout.clone();
    assert!(!String::from_utf8(output).unwrap().contains("To undo"));
    let seen = requested(&requests);
    assert!(seen.iter().all(|path| !path.contains("adopt")), "{seen:?}");
    assert_eq!(
        before,
        (
            fs::read(&rules).unwrap(),
            fs::metadata(&rules).unwrap().modified().unwrap()
        )
    );
}

#[cfg(unix)]
#[test]
fn setup_leaves_a_blocked_plan_alone_and_still_succeeds() {
    let state = tempfile::tempdir().unwrap();
    let mut plan = setup_home(state.path());
    plan.note_codes
        .push(wenlan_types::rulesync::RulesSetupCode::SkillPreviewFailed.into());
    plan.notes.push("preview failed".into());
    let (mut cmd, requests) = setup_fixture(state.path(), false, &plan);
    let output = cmd.assert().success().get_output().stdout.clone();
    let text = String::from_utf8(output).unwrap();
    assert!(
        text.contains("Your tool setup could not be fully set up"),
        "{text}"
    );
    assert!(!text.contains("To undo"), "{text}");
    let seen = requested(&requests);
    assert!(
        seen.iter()
            .all(|path| path != "POST /api/rules/setup/adopt"),
        "{seen:?}"
    );
}

#[test]
fn personal_rules_dry_run_previews_safe_settings_only_with_the_flag() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    fs::create_dir_all(home.join(".claude")).unwrap();
    let path = home.join(".claude.json");
    let original = br#"{"mcpServers":{"visible":{"command":"fixture-adopt"},"held":{"command":"fixture-hidden","env":{"MY_LOGIN":"synthetic-only-sentinel"}}}}"#;
    fs::write(&path, original).unwrap();
    for enabled in [false, true] {
        let mut cmd = personal_rules_cli(state.path());
        if enabled {
            cmd.env("WENLAN_RULESYNC", "1");
        }
        let output = cmd
            .args(["rules", "adopt", "--dry-run", "--format", "json"])
            .assert()
            .success();
        let stdout = std::str::from_utf8(&output.get_output().stdout).unwrap();
        let plan: AdoptionPlan = serde_json::from_str(stdout).unwrap();
        assert_eq!(
            plan.files
                .iter()
                .any(|file| file.path == ".claude.json" && file.diff.contains("fixture-adopt")),
            enabled
        );
        assert_eq!(
            plan.kept_in_place,
            if enabled {
                vec![wenlan_types::rulesync::KeptInPlace {
                    tool: "claudecode".into(),
                    count: 1,
                }]
            } else {
                Vec::new()
            }
        );
        assert!(!stdout.contains("synthetic-only-sentinel"));
        assert!(!stdout.contains("fixture-hidden"));
        assert_eq!(fs::read(&path).unwrap(), original);
        assert!(!state.path().join("unused-data-root").exists());
    }
}

#[test]
fn personal_rules_dry_run_reads_existing_files_without_daemon_or_data_root() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    fs::create_dir_all(home.join(".agents")).unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    let master = home.join(".agents/AGENTS.md");
    let codex = home.join(".codex/AGENTS.md");
    fs::write(&master, "Keep shared rules.\n").unwrap();
    fs::write(&codex, "Keep Codex rules.\n").unwrap();

    let output = personal_rules_cli(state.path())
        .env("WENLAN_DEFAULT_SPACE", "Clinic")
        .args(["rules", "adopt", "--dry-run", "--format", "json"])
        .assert()
        .success();
    let plan: AdoptionPlan = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let change = plan
        .files
        .iter()
        .find(|change| Path::new(&change.path) == Path::new(".codex/AGENTS.md"))
        .expect("Codex migration plan");
    assert_eq!((change.added, change.removed), (1, 0));
    assert!(change.diff.contains("+Keep shared rules."));
    assert_eq!(fs::read_to_string(master).unwrap(), "Keep shared rules.\n");
    assert_eq!(fs::read_to_string(codex).unwrap(), "Keep Codex rules.\n");
    assert!(!home.join(".claude").exists());
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn personal_rules_dry_run_reports_extended_targets_only_with_the_flag() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    let rules = ".gemini/GEMINI.md";
    fs::create_dir_all(home.join(".gemini")).unwrap();
    fs::write(home.join(rules), "Gemini only.\r\n").unwrap();

    for flag in [None, Some("1")] {
        let mut cmd = personal_rules_cli(state.path());
        if let Some(flag) = flag {
            cmd.env("WENLAN_RULESYNC", flag);
        }
        let output = cmd
            .args(["rules", "adopt", "--dry-run", "--format", "json"])
            .assert()
            .success();
        let plan: AdoptionPlan = serde_json::from_slice(&output.get_output().stdout).unwrap();
        assert_eq!(
            plan.files.iter().any(|file| file.path == rules),
            flag.is_some()
        );
        assert_eq!(
            plan.notes
                .iter()
                .any(|note| note.starts_with("Skipped Antigravity ")),
            flag.is_some()
        );
        if flag.is_some() {
            assert!(plan.notes[0].starts_with("Offline inventory"));
            assert!(plan.token.is_empty());
        }
    }
    assert_eq!(fs::read(home.join(rules)).unwrap(), b"Gemini only.\r\n");
    assert!(!home.join(".gemini/config").exists());
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn ux_skill_18_personal_rules_dry_run_lists_commands_and_agents_only_with_the_flag() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    let sources = [
        (
            ".claude/commands/check.md",
            "---\ndescription: Check\nallowed-tools: Read\n---\nCheck $ARGUMENTS.\n",
        ),
        (
            ".claude/agents/review.md",
            "---\nname: review\ndescription: Review\nmodel: sonnet\ntools: Read\n---\nReview.\n",
        ),
        (
            ".codex/agents/review.toml",
            "name = \"review\"\ndescription = \"Review\"\ndeveloper_instructions = \"Review.\"\n",
        ),
    ];
    for (path, bytes) in sources {
        let file = home.join(path);
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(file, bytes).unwrap();
    }
    fs::write(home.join(".cursorignore"), "project-only\n").unwrap();

    for enabled in [false, true] {
        let mut cmd = personal_rules_cli(state.path());
        if enabled {
            cmd.env("WENLAN_RULESYNC", "1");
        }
        let output = cmd
            .args(["rules", "adopt", "--dry-run", "--format", "json"])
            .assert()
            .success();
        let plan: AdoptionPlan = serde_json::from_slice(&output.get_output().stdout).unwrap();
        for (path, bytes) in sources {
            assert_eq!(
                plan.files.iter().any(|change| change.path == path),
                enabled,
                "{path}"
            );
            assert_eq!(fs::read_to_string(home.join(path)).unwrap(), bytes);
        }
        assert!(!plan
            .files
            .iter()
            .any(|change| change.path == ".cursorignore"));
        assert_eq!(
            plan.notes
                .iter()
                .any(|note| note.contains("Personal ignore: none supported")),
            enabled
        );
    }
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn personal_rules_dry_run_labels_skill_sources() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    for path in [".agents/skills/locked", ".codex/skills/handmade"] {
        let skill = home.join(path);
        fs::create_dir_all(&skill).unwrap();
        let name = skill.file_name().unwrap().to_str().unwrap();
        fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: Fixture skill\n---\nSkill contents\n"),
        )
        .unwrap();
    }
    fs::write(home.join(".agents/.skill-lock.json"), br#"{"skills":{"locked":{
        "sourceType":"github","source":"owner/project","sourceUrl":"https://github.com/owner/project.git",
        "skillPath":"library/locked/SKILL.md"
    }}}"#).unwrap();
    personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            ".agents/skills/locked — skipped (Wenlan reads this shared source",
        ))
        .stdout(predicate::str::contains(
            ".codex/skills/handmade — plain copy in the personal master",
        ));
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn personal_rules_dry_run_shows_the_rendered_skill() {
    let state = tempfile::tempdir().unwrap();
    let folder = state.path().join("ai-tools/home/.codex/skills/folding");
    fs::create_dir_all(&folder).unwrap();
    let initial = "---\nname: folding\ndescription: A long description with enough words to make the standard formatter wrap it over multiple lines while retaining every word.\n---\n\nBody.\n";
    let path = folder.join("SKILL.md");
    fs::write(&path, initial).unwrap();
    personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("+description: >-\n"))
        .stdout(predicate::str::contains(
            "+  over multiple lines while retaining every word.\n",
        ));
    assert_eq!(fs::read_to_string(path).unwrap(), initial);
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn personal_rules_dry_run_announces_retired_codex_script() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    let script = home.join(".agents/scripts/sync-codex-agents.sh");
    let codex = home.join(".codex/AGENTS.md");
    fs::create_dir_all(script.parent().unwrap()).unwrap();
    fs::create_dir_all(codex.parent().unwrap()).unwrap();
    let old = "<!-- Generated by ~/.agents/scripts/sync-codex-agents.sh. Edit the adapter, then rerun. -->";
    fs::write(&script, "#!/bin/sh\n").unwrap();
    let original = format!("{old}\nKeep my Codex rules.\n");
    fs::write(&codex, &original).unwrap();

    let output = personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "json"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&output.get_output().stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.get_output().stderr).into_owned();
    let plan: AdoptionPlan = serde_json::from_str(&stdout)
        .unwrap_or_else(|error| panic!("plan is not JSON ({error}):\n{stdout}\nstderr:\n{stderr}"));
    let paths: Vec<&str> = plan.files.iter().map(|file| file.path.as_str()).collect();
    let context = format!(
        "plan.files paths: {paths:?}\nplan.notes: {:#?}\nfull plan:\n{stdout}\nstderr:\n{stderr}",
        plan.notes
    );
    let diff = &plan
        .files
        .iter()
        .find(|file| file.path == ".codex/AGENTS.md")
        .unwrap_or_else(|| panic!("no .codex/AGENTS.md in the plan\n{context}"))
        .diff;
    assert!(
        diff.lines()
            .any(|line| line.starts_with("+<!-- Managed by Wenlan.")),
        "no managed header added\ndiff:\n{diff:?}\n{context}"
    );
    assert!(
        diff.contains(&format!("-{old}")),
        "legacy header not removed\ndiff:\n{diff:?}\n{context}"
    );
    assert!(
        !diff.contains(&format!("+{old}")),
        "legacy header kept\ndiff:\n{diff:?}\n{context}"
    );
    assert!(
        plan.notes
            .first()
            .is_some_and(|note| note.contains("sync-codex-agents.sh becomes unused")),
        "retired script not announced\n{context}"
    );
    personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "script stays in place for you to keep or remove",
        ));
    assert_eq!(fs::read_to_string(codex).unwrap(), original);
    assert_eq!(fs::read_to_string(script).unwrap(), "#!/bin/sh\n");
}

#[test]
fn personal_rules_dry_run_leaves_an_oversized_skill_unmanaged() {
    let state = tempfile::tempdir().unwrap();
    let relative = ".codex/skills/video-shotcraft";
    let skill = state.path().join("ai-tools/home").join(relative);
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("SKILL.md"), "An existing skill\n").unwrap();
    let audio = skill.join("audio.mp3");
    let size = 32 * 1024 * 1024;
    // The audio file is within the limit by itself. The whole skill exceeds it.
    fs::File::create(&audio).unwrap().set_len(size).unwrap();

    let output = personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "json"])
        .assert()
        .success();
    let plan: AdoptionPlan = serde_json::from_slice(&output.get_output().stdout).unwrap();
    let item = plan
        .skills
        .iter()
        .find(|item| item.path == relative)
        .unwrap();
    let AdoptionSkillOutcome::Skipped { reason } = &item.outcome else {
        panic!("expected an unmanaged skip, got {:?}", item.outcome);
    };
    assert!(reason.contains("32 MiB total import limit"));
    assert!(reason.contains("audio.mp3"));
    let changes = plan
        .files
        .iter()
        .filter(|file| Path::new(&file.path).starts_with(relative))
        .collect::<Vec<_>>();
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].path, relative);
    assert_eq!((changes[0].added, changes[0].removed), (0, 0));
    assert!(changes[0].diff.contains("Stays where it is, not managed."));
    personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("stays where it is, not managed"));
    assert_eq!(fs::metadata(&audio).unwrap().len(), size);
    assert_eq!(
        fs::read_to_string(skill.join("SKILL.md")).unwrap(),
        "An existing skill\n"
    );
    assert!(!state.path().join("unused-data-root").exists());
}

fn session_rules_read(args: &[&str], sync: bool, named: bool) {
    let state = tempfile::tempdir().unwrap();
    let data = state.path().join("unused-data-root");
    fs::create_dir_all(&data).unwrap();
    fs::write(data.join("person.key"), "a".repeat(64)).unwrap();
    let mut routes = vec![("GET /api/health", "{}".to_string())];
    if sync {
        routes.push((
            "POST /api/rules/setup/sync",
            json!({"written":[],"blocked":[],"proposals":[],"message":"done"}).to_string(),
        ));
        routes.push(("GET /api/health", "{}".to_string()));
    }
    let (route, payload) = if args.contains(&"review") {
        (
            "GET /api/rules/setup/review",
            serde_json::to_string(&wenlan_types::rulesync::RulesReviewList::default()).unwrap(),
        )
    } else {
        (
            "GET /api/rules/setup/status",
            serde_json::to_string(&wenlan_types::rulesync::RulesSetupStatus::default()).unwrap(),
        )
    };
    routes.push((route, payload));
    let (base, _) = spawn_stub(routes);
    let mut cmd = personal_rules_cli(state.path());
    cmd.env("WENLAN_HOST", base)
        .env("WENLAN_RULESYNC", "1")
        .args(args);
    if named {
        cmd.env("WENLAN_AGENT_NAME", "fixture-agent");
    } else {
        for_agent_session(&mut cmd);
    }
    let assertion = cmd.assert().success();
    if sync {
        assertion.stderr(predicate::str::contains("done"));
    }
}

fn for_agent_session(cmd: &mut Command) -> &mut Command {
    cmd.env("WENLAN_SESSION", "codex:fixture")
}

#[test]
fn rules_status_allowed_in_session() {
    session_rules_read(&["rules", "status"], false, false);
    session_rules_read(&["rules", "status"], false, true);
}
#[test]
fn rules_status_sync_allowed_in_session() {
    session_rules_read(&["rules", "status", "--sync"], true, false);
    session_rules_read(&["rules", "status", "--sync"], true, true);
}
#[test]
fn rules_review_list_allowed_in_session() {
    session_rules_read(&["rules", "review"], false, false);
    session_rules_read(&["rules", "review"], false, true);
}

fn session_rules_refused(args: &[&str]) {
    let state = tempfile::tempdir().unwrap();
    for_agent_session(personal_rules_cli(state.path()).args(args)).assert().failure().stderr(predicate::str::contains("Only you can change your personal tool setup. Run this from your own terminal outside an AI session."));
}
#[test]
fn rules_adopt_in_session_needs_confirmed_plan() {
    let state = tempfile::tempdir().unwrap();
    for_agent_session(personal_rules_cli(state.path()).args(["rules", "adopt"]))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "after they say yes run `wenlan rules adopt --confirm <plan id>`",
        ));
}
#[test]
fn rules_confirmed_adoption_sends_previewed_plan_in_session() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::create_dir_all(state.path().join("unused-data-root")).unwrap();
    fs::write(
        state.path().join("unused-data-root/person.key"),
        "a".repeat(64),
    )
    .unwrap();
    fs::write(home.join(".codex/AGENTS.md"), "Fixture rules.\n").unwrap();
    let plan = wenlan_core::rulesync_personal::plan_adoption(&home).unwrap();
    let (base, requests) = spawn_stub(vec![
        ("GET /api/health", "{}".into()),
        (
            "POST /api/rules/setup/adopt/preview",
            serde_json::to_string(&plan).unwrap(),
        ),
        ("GET /api/health", "{}".into()),
        (
            "POST /api/rules/setup/adopt",
            json!({"written":[],"blocked":[],"proposals":[],"message":"done"}).to_string(),
        ),
    ]);
    for_agent_session(
        personal_rules_cli(state.path())
            .env("WENLAN_HOST", base)
            .env("WENLAN_RULESYNC", "1")
            .args(["rules", "adopt", "--confirm", &plan.plan_id]),
    )
    .assert()
    .success();
    let bodies: Vec<_> = requests.try_iter().collect();
    let body = &bodies
        .iter()
        .find(|(path, _)| path == "POST /api/rules/setup/adopt")
        .unwrap()
        .1;
    let request: wenlan_types::rulesync::AdoptRequest = serde_json::from_str(body).unwrap();
    assert_eq!(
        request.confirmed_plan_id.as_deref(),
        Some(plan.plan_id.as_str())
    );
    assert_eq!(request.token, plan.token);
}

#[test]
fn rules_confirmed_adoption_refuses_changed_plan_in_session() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    let skill = home.join(".codex/skills/example/SKILL.md");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    fs::create_dir_all(state.path().join("unused-data-root")).unwrap();
    fs::write(
        state.path().join("unused-data-root/person.key"),
        "a".repeat(64),
    )
    .unwrap();
    fs::write(
        &skill,
        "---\nname: example\ndescription: Fixture\n---\nOld.\n",
    )
    .unwrap();
    let old = wenlan_core::rulesync_personal::plan_adoption(&home).unwrap();
    let changed = "---\nname: example\ndescription: Fixture\n---\nChanged.\n";
    fs::write(&skill, changed).unwrap();
    let fresh = wenlan_core::rulesync_personal::plan_adoption(&home).unwrap();
    assert_ne!(old.plan_id, fresh.plan_id);
    let (base, requests) = spawn_stub(vec![
        ("GET /api/health", "{}".into()),
        (
            "POST /api/rules/setup/adopt/preview",
            serde_json::to_string(&fresh).unwrap(),
        ),
    ]);
    for_agent_session(
        personal_rules_cli(state.path())
            .env("WENLAN_HOST", base)
            .env("WENLAN_RULESYNC", "1")
            .args(["rules", "adopt", "--confirm", &old.plan_id]),
    )
    .assert()
    .failure()
    .stderr(predicate::str::contains(
        "The setup changed since that preview",
    ));
    assert!(requests
        .try_iter()
        .all(|(path, _)| path != "POST /api/rules/setup/adopt"));
    assert_eq!(fs::read_to_string(skill).unwrap(), changed);
    assert!(!state.path().join("unused-data-root/personal.json").exists());
}

/// A stub daemon whose share preview returns `plan`, then accepts a share.
fn share_fixture(state: &Path) -> (Command, std::sync::mpsc::Receiver<(String, String)>) {
    fs::create_dir_all(state.join("ai-tools/home")).unwrap();
    fs::create_dir_all(state.join("unused-data-root")).unwrap();
    fs::write(state.join("unused-data-root/person.key"), "a".repeat(64)).unwrap();
    let plan = json!({
        "plan_id": "0123456789ab",
        "token": "fixture-token",
        "tools": [{"tool": "Codex", "skills": [], "commands": ["hello"], "subagents": [], "mcp_servers": ["docs"]}],
        "stays": [{"tool": "Claude Code", "item": "an MCP server in .claude.json", "reason": "its name may contain a key"}],
        "already_there": 2,
        "key_servers": ["billing", "search"],
        "key_tools": ["Codex", "Cursor"],
        "rules": [
            {"number": 1, "line": "Run the release checklist.", "tools": ["Claude Code"], "kept": false},
            {"number": 2, "line": format!("Review {} for changes.", "規".repeat(60)), "tools": ["Claude Code"], "kept": false},
            {"number": 3, "line": "Keep the draft checklist here.", "tools": ["Claude Code"], "kept": true}
        ],
        "rule_stays": [
            {"tool": "Claude Code", "reason": "This line may hold a credential."},
            {"tool": "Codex", "line": "Use Codex for local work.", "reason": "This line names a tool."}
        ]
    });
    let (base, requests) = spawn_stub(vec![
        ("GET /api/health", "{}".into()),
        ("POST /api/rules/setup/share/preview", plan.to_string()),
        ("GET /api/health", "{}".into()),
        (
            "POST /api/rules/setup/share",
            json!({
                "written": [],
                "blocked": [],
                "proposals": [],
                "message": "shared",
                "kept_in_place": [
                    {"tool": "Claude Code", "count": 2},
                    {"tool": "Codex CLI", "count": 3}
                ]
            })
            .to_string(),
        ),
    ]);
    let mut cmd = personal_rules_cli(state);
    cmd.env("WENLAN_HOST", base).env("WENLAN_RULESYNC", "1");
    (cmd, requests)
}

fn shared(requests: &std::sync::mpsc::Receiver<(String, String)>) -> Option<String> {
    requests
        .try_iter()
        .find(|(path, _)| path == "POST /api/rules/setup/share")
        .map(|(_, body)| body)
}

#[test]
fn rules_share_in_session_without_confirm_only_previews() {
    let state = tempfile::tempdir().unwrap();
    let (mut cmd, requests) = share_fixture(state.path());
    // An AI session reads the plan as JSON.
    for_agent_session(cmd.args(["rules", "share", "--dry-run"]))
        .assert()
        .success()
        .stdout(predicate::str::contains("\"plan_id\": \"0123456789ab\""))
        .stdout(predicate::str::contains("its name may contain a key"))
        .stdout(predicate::str::contains("\"key_servers\""));
    assert!(shared(&requests).is_none());

    // The person sees the same plan in words.
    let (mut cmd, requests) = share_fixture(state.path());
    cmd.args(["rules", "share", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Share with your other tools"))
        .stdout(predicate::str::contains("Codex: 1 command, 1 MCP server"))
        .stdout(predicate::str::contains("  + command hello").not())
        .stdout(predicate::str::contains("2 items already in every tool"))
        .stdout(predicate::str::contains(
            "1 item stays in its own tool: 1 may hold credentials.",
        ))
        .stdout(predicate::str::contains(
            "These 2 servers may contain a key. Sharing copies the key into: Codex, Cursor.",
        ))
        .stdout(predicate::str::contains("  ? MCP server billing"))
        .stdout(predicate::str::contains("Add --details to see every item."))
        .stdout(predicate::str::contains(
            "wenlan rules share --confirm 0123456789ab",
        ));
    assert!(shared(&requests).is_none());

    let (mut cmd, requests) = share_fixture(state.path());
    cmd.args([
        "rules",
        "share",
        "--dry-run",
        "--details",
        "--format",
        "table",
    ])
    .assert()
    .success()
    .stdout(predicate::str::contains("  + command hello"))
    .stdout(predicate::str::contains(
        "Stays in Claude Code: an MCP server in .claude.json: its name may contain a key",
    ));
    assert!(shared(&requests).is_none());

    let (mut cmd, requests) = share_fixture(state.path());
    for_agent_session(cmd.args(["rules", "share"]))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "after they say yes run `wenlan rules share --confirm <plan id>`",
        ));
    assert!(requests.try_iter().next().is_none());
}

#[test]
fn rules_share_in_session_applies_the_confirmed_plan() {
    let state = tempfile::tempdir().unwrap();
    let (mut cmd, requests) = share_fixture(state.path());
    for_agent_session(cmd.args(["rules", "share", "--confirm", "0123456789ab"]))
        .assert()
        .success();
    let request: wenlan_types::rulesync::ShareRequest =
        serde_json::from_str(&shared(&requests).unwrap()).unwrap();
    assert_eq!(
        request.approval.confirmed_plan_id.as_deref(),
        Some("0123456789ab")
    );
    assert_eq!(request.approval.token, "fixture-token");
    assert_eq!(request.keep_rule, Some(vec![]));
    // The AI confirm route never asks for the servers that may hold a key.
    assert!(!request.approval.include_keys);
}

#[test]
fn rules_share_short_preview_shows_each_moving_line_at_terminal_width() {
    let state = tempfile::tempdir().unwrap();
    let (mut cmd, requests) = share_fixture(state.path());
    let assertion =
        for_agent_session(cmd.args(["rules", "share", "--dry-run", "--format", "table"]))
            .assert()
            .success();
    let output = std::str::from_utf8(&assertion.get_output().stdout).unwrap();
    let lines: Vec<_> = output.lines().collect();
    assert!(
        lines.contains(&"  1. Run the release checklist."),
        "{output}"
    );
    let long = lines
        .iter()
        .find(|line| line.starts_with("  2. Review "))
        .unwrap();
    assert!(
        long.ends_with('…') && console::measure_text_width(long) <= 100,
        "{long}"
    );
    assert!(
        lines.contains(
            &"2 rule lines stay in their own tools (1 tool-only, 1 with possible credentials)."
        ),
        "{output}"
    );
    assert!(
        !output.contains("Keep the draft checklist here."),
        "{output}"
    );
    assert!(!output.contains("Use Codex for local work."), "{output}");
    assert!(shared(&requests).is_none());
}

#[test]
fn ux_skill_26_rules_share_keep_rule_reaches_preview_and_confirm() {
    let state = tempfile::tempdir().unwrap();
    let (mut cmd, requests) = share_fixture(state.path());
    cmd.args([
        "--session",
        "claude-code:synthetic-rule-share",
        "rules",
        "share",
        "--confirm",
        "0123456789ab",
        "--keep-rule",
        "1,3",
    ])
    .assert()
    .success();
    let requests: Vec<_> = requests.try_iter().collect();
    for endpoint in [
        "POST /api/rules/setup/share/preview",
        "POST /api/rules/setup/share",
    ] {
        let (_, body) = requests.iter().find(|(path, _)| path == endpoint).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(body).unwrap()["keep_rule"],
            json!([1, 3])
        );
    }
}

#[test]
fn rules_share_reports_kept_in_place_counts_once() {
    let state = tempfile::tempdir().unwrap();
    let (mut cmd, requests) = share_fixture(state.path());
    let output = cmd
        .args([
            "--format",
            "table",
            "rules",
            "share",
            "--confirm",
            "0123456789ab",
        ])
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "shared\nKept 5 settings in place in their own tools because they may hold keys or cannot be copied.\n"
    );
    assert!(shared(&requests).is_some());
}

#[test]
fn rules_share_in_session_refuses_a_stale_plan_id() {
    let state = tempfile::tempdir().unwrap();
    let (mut cmd, requests) = share_fixture(state.path());
    for_agent_session(cmd.args(["rules", "share", "--confirm", "ba9876543210"]))
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "Run `wenlan rules share --dry-run` again",
        ));
    assert!(shared(&requests).is_none());
}

#[test]
fn rules_restore_refused_in_session() {
    session_rules_refused(&["rules", "restore"]);
}
#[test]
fn rules_review_answer_refused_in_session() {
    session_rules_refused(&["rules", "review", "card-1", "all-tools"]);
}

#[test]
fn personal_rules_setup_refuses_the_strict_space_pin() {
    let state = tempfile::tempdir().unwrap();
    personal_rules_cli(state.path())
        .env("WENLAN_SPACE", "Clinic")
        .args(["rules", "adopt", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "WENLAN_SPACE limits this terminal",
        ));
}

#[test]
fn personal_rules_setup_requires_interactive_confirmation() {
    let state = tempfile::tempdir().unwrap();
    for args in [
        &["rules", "adopt"][..],
        &["rules", "share"],
        &["rules", "restore"],
        &["rules", "settings", "local-settings"],
        &[
            "rules",
            "review",
            "card-1",
            "all-tools",
            "--file",
            "rules.md",
        ],
    ] {
        personal_rules_cli(state.path())
            .args(args)
            .write_stdin("yes\n")
            .assert()
            .failure()
            .stderr(predicate::str::contains("from your own terminal"));
        personal_rules_cli(state.path())
            .args(args)
            .arg("--yes")
            .assert()
            .failure()
            .stderr(predicate::str::contains("unexpected argument '--yes'"));
    }
}

#[test]
fn personal_rules_settings_preview_refuses_non_local_daemon() {
    let state = tempfile::tempdir().unwrap();
    personal_rules_cli(state.path())
        .env("WENLAN_HOST", "http://192.0.2.1:17917")
        .args(["rules", "settings", "local-settings", "--dry-run"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("daemon on this computer"));
    assert!(!state.path().join("unused-data-root").exists());
}

fn saved() -> String {
    json!({
        "status": "saved",
        "rule_id": "rule0123456789abcdef",
        "space_name": "Clinic",
        "page_id": "page_1",
        "page_title": "Rules I saved — Clinic",
        "saved_from": "terminal",
        "message": "Saved for the Space \"Clinic\". Your AI tools are told this rule from now on.",
    })
    .to_string()
}

/// The Spaces the stand-in daemon knows; the CLI asks before every command
/// that works in a Space.
fn spaces() -> (&'static str, String) {
    (
        "GET /api/spaces",
        json!([{
            "id": "space_1", "name": "Clinic", "description": null,
            "suggested": false, "starred": false, "sort_order": 0,
            "memory_count": 0, "entity_count": 0, "created_at": 0.0, "updated_at": 0.0,
        }])
        .to_string(),
    )
}

/// What the command sent, after asking for the Spaces.
fn sent(
    received: &std::sync::mpsc::Receiver<(String, String)>,
) -> Vec<(String, serde_json::Value)> {
    received
        .try_iter()
        .filter(|(route, _)| route != "GET /api/spaces")
        .map(|(route, body)| {
            let body = serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);
            (route, body)
        })
        .collect()
}

/// UX-CLI-1: `wenlan rules add` sends the daemon the same request on the
/// same route as an AI tool's `add_rule` (its twin test is in
/// `wenlan-mcp/src/rules_tool_tests.rs`). Only the words asked differ, and
/// where the Space travels: here as the connection's Space, like every
/// other command.
#[test]
fn ux_cli_1_rules_add_matches_add_rule() {
    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .args(["--format", "table", "--space", "Clinic"])
        .args(["rules", "add", "We release on Tuesdays."])
        .assert()
        .success()
        .stdout("Saved for the Space \"Clinic\". Your AI tools are told this rule from now on.\n");
    assert_eq!(
        sent(&received),
        vec![(
            "POST /api/skills/rules".to_string(),
            json!({
                "text": "We release on Tuesdays.",
                "person_asked": "wenlan rules add \"We release on Tuesdays.\"",
            })
        )]
    );
}

/// UX-SKILL-14: a rule is for every Space only by its own flag, and the
/// flag and a named Space together are refused before anything is sent.
#[test]
fn ux_skill_14_every_space_is_only_ever_explicit() {
    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .success();
    assert_eq!(
        sent(&received),
        vec![(
            "POST /api/skills/rules".to_string(),
            json!({
                "text": "Answer in plain words.",
                "every_space": true,
                "person_asked": "wenlan rules add \"Answer in plain words.\" --every-space",
            })
        )]
    );

    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .args(["--space", "Clinic"])
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .failure();
    assert!(sent(&received).is_empty(), "nothing was sent");
}

/// UX-SKILL-14: `WENLAN_SPACE` limits this terminal to one Space. A rule
/// for every Space is then refused before anything is sent; a rule for that
/// Space goes out under the Space, so the daemon holds it there. Without the
/// limit, a rule for every Space carries no Space at all, whatever Space
/// the folder or the default would give.
#[test]
fn ux_skill_14_every_space_is_refused_under_the_space_lock() {
    let (base, received) = spawn_stub(vec![spaces(), ("POST /api/skills/rules", saved())]);
    cli(&base)
        .env("WENLAN_SPACE", "Clinic")
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "WENLAN_SPACE limits this terminal to the Space 'Clinic'",
        ))
        .stderr(predicate::str::contains("Nothing was saved"));
    assert!(
        received.try_iter().next().is_none(),
        "nothing at all was sent"
    );

    // For its own Space the locked terminal saves, and says which Space.
    let (base, received) = spawn_headers_stub("POST /api/skills/rules", saved());
    cli(&base)
        .env("WENLAN_SPACE", "Clinic")
        .args(["rules", "add", "We release on Tuesdays."])
        .assert()
        .success();
    let headers = received.recv().expect("the save");
    assert!(headers.contains("x-wenlan-space: clinic"), "{headers}");

    // Not locked: every Space carries no Space, even with a default set.
    let (base, received) = spawn_headers_stub("POST /api/skills/rules", saved());
    cli(&base)
        .env("WENLAN_DEFAULT_SPACE", "Clinic")
        .args(["rules", "add", "Answer in plain words.", "--every-space"])
        .assert()
        .success();
    let headers = received.recv().expect("the save");
    assert!(!headers.contains("x-wenlan-space"), "{headers}");
}

/// A stand-in daemon that answers `GET /api/spaces` and one other route,
/// and hands back that other request's headers, lower-cased.
fn spawn_headers_stub(
    route: &'static str,
    payload: String,
) -> (String, std::sync::mpsc::Receiver<String>) {
    use std::io::{BufRead, BufReader, Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind stub");
    let base = format!("http://{}", listener.local_addr().expect("stub address"));
    let (sent, received) = std::sync::mpsc::channel();
    let spaces = spaces().1;
    std::thread::spawn(move || {
        for stream in listener.incoming().take(2).flatten() {
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).expect("request line");
            let (mut headers, mut length) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                let line = line.to_lowercase();
                if let Some(value) = line.strip_prefix("content-length:") {
                    length = value.trim().parse().unwrap_or(0);
                }
                headers.push_str(&line);
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).expect("body");
            let is_route = request_line.starts_with(route);
            let payload = if is_route { &payload } else { &spaces };
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{payload}",
                payload.len()
            );
            if is_route {
                let _ = sent.send(headers);
            }
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .expect("answer");
            if is_route {
                break;
            }
        }
    });
    (base, received)
}

/// UX-SKILL-14: removing is one command with the id `wenlan rules` prints.
#[test]
fn ux_skill_14_rules_remove_asks_the_daemon_to_remove_that_rule() {
    let removed = json!({
        "status": "removed",
        "id": "rule0123456789abcdef",
        "page_id": "page_1",
        "page_title": "Rules I saved — Clinic",
        "message": "Removed. Your AI tools are no longer told this rule.",
    })
    .to_string();
    let (base, received) = spawn_stub(vec![
        spaces(),
        (
            "POST /api/skills/rules/rule0123456789abcdef/remove",
            removed,
        ),
    ]);
    cli(&base)
        .args(["--format", "table"])
        .args(["rules", "remove", "rule0123456789abcdef"])
        .assert()
        .success()
        .stdout("Removed. Your AI tools are no longer told this rule.\n");
    assert_eq!(
        sent(&received),
        vec![(
            "POST /api/skills/rules/rule0123456789abcdef/remove".to_string(),
            json!({"person_asked": "wenlan rules remove rule0123456789abcdef"})
        )]
    );

    // Something that is not an id never reaches the daemon.
    let (base, received) = spawn_stub(vec![
        spaces(),
        ("POST /api/skills/rules/x/remove", String::new()),
    ]);
    cli(&base)
        .args(["rules", "remove", "../pages"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("not a rule id"));
    assert!(sent(&received).is_empty(), "nothing was sent");
}

/// Plain `wenlan rules` still reads the rules, and a saved rule says where
/// it was saved from and how to take it out (UX-SKILL-14).
#[test]
fn ux_skill_14_plain_rules_lists_a_saved_rule_with_where_it_came_from() {
    let rules = json!({
        "space_id": "space_1",
        "space_name": "Clinic",
        "lines": [{
            "id": "rule0123456789abcdef",
            "kind": "rule",
            "text": "We release on Tuesdays.",
            "source_page_id": "page_1",
            "source_page_title": "Rules I saved — Clinic",
            "source_revision": 2,
            "source_revision_at": 1_794_000_000,
            "source_quote": "We release on Tuesdays.",
            "saved_from": "Codex",
        }, {
            "id": "rule1123456789abcdef",
            "kind": "rule",
            "text": "Answer in plain words.",
            "source_page_id": "page_2",
            "source_page_title": "Rules I saved",
            "source_revision": 1,
            "source_revision_at": 1_794_000_000,
            "source_quote": "Answer in plain words.",
            "general": true,
            "saved_from": "terminal",
        }],
    })
    .to_string();
    let (base, received) = spawn_stub(vec![spaces(), ("GET /api/skills/rules", rules)]);
    cli(&base)
        .args(["--format", "table", "rules"])
        .assert()
        .success()
        .stdout(
            "Working rules for Clinic\n\n\
             - We release on Tuesdays.\n  \
             saved from Codex, kept in \"Rules I saved — Clinic\"\n  \
             to remove it: wenlan rules remove rule0123456789abcdef\n\
             - Answer in plain words.\n  \
             saved from the terminal, kept in \"Rules I saved\", used in every Space\n  \
             to remove it: wenlan rules remove rule1123456789abcdef\n",
        )
        .stdout(predicate::str::contains("written by you").not());
    assert_eq!(sent(&received)[0].0, "GET /api/skills/rules");
}

#[cfg(unix)]
#[test]
fn personal_rules_dry_run_names_git_destinations_and_plans_codex_link_replacement() {
    let state = tempfile::tempdir().unwrap();
    let home = state.path().join("ai-tools/home");
    for repo in [".agents", ".claude"] {
        let folder = home.join(repo).join("skills/sample");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("SKILL.md"),
            "---\nname: sample\ndescription: Fixture skill\n---\n\nInstructions.\n",
        )
        .unwrap();
    }
    for (root, name) in [(".agents", "agents-harness"), (".claude", "claude-config")] {
        let git = |args: &[&str]| {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(home.join(root))
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        git(&["init", "--quiet"]);
        git(&[
            "remote",
            "add",
            "origin",
            &format!("https://github.com/fixture/{name}.git"),
        ]);
    }
    let codex = home.join(".codex/skills");
    fs::create_dir_all(&codex).unwrap();
    std::os::unix::fs::symlink("../../.agents/skills/sample", codex.join("sample")).unwrap();
    personal_rules_cli(state.path())
        .args(["rules", "adopt", "--dry-run", "--format", "table"])
        .assert()
        .success()
        .stdout(predicate::str::contains("~/.agents is left untouched."))
        .stdout(predicate::str::contains(
            "~/.agents/skills is inside the Git repository agents-harness. Wenlan reads the shared skills there and leaves that directory in place.",
        ))
        .stdout(predicate::str::contains(
            "~/.claude/skills is inside the Git repository claude-config. Wenlan will manage 1 skill folder there, and its writes can appear as changes in that repository.",
        ))
        .stdout(predicate::str::contains(
            ".codex/skills/sample — plain copy in the personal master",
        ))
        .stdout(predicate::str::contains("symlink -> ../../.agents/skills/sample"));
    assert!(codex.join("sample").is_symlink());
    assert!(!state.path().join("unused-data-root").exists());
}

#[test]
fn rules_named_agent_refuses_person_changes() {
    for args in [
        &["rules", "adopt"][..],
        &["rules", "restore"][..],
        &["rules", "review", "card", "dismiss"][..],
    ] {
        let state = tempfile::tempdir().unwrap();
        personal_rules_cli(state.path())
            .env("WENLAN_AGENT_NAME", "fixture-agent")
            .args(args)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "Only you can change your personal tool setup",
            ));
    }
}

#[test]
fn rules_status_sends_caller_identity() {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;
    for (name, value, expected) in [
        (
            "WENLAN_SESSION",
            "codex:fixture",
            "x-wenlan-session: codex:fixture",
        ),
        (
            "WENLAN_AGENT_NAME",
            "fixture-agent",
            "x-agent-name: fixture-agent",
        ),
    ] {
        let state = tempfile::tempdir().unwrap();
        let data = state.path().join("unused-data-root");
        fs::create_dir_all(&data).unwrap();
        fs::write(data.join("person.key"), "a".repeat(64)).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming().take(2) {
                let stream = stream.unwrap();
                let mut reader = BufReader::new(stream);
                let mut headers = String::new();
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    headers.push_str(&line);
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                }
                let payload = if headers.starts_with("GET /api/rules/setup/status ") {
                    tx.send(headers.to_lowercase()).unwrap();
                    serde_json::to_string(&wenlan_types::rulesync::RulesSetupStatus::default())
                        .unwrap()
                } else {
                    "{}".to_string()
                };
                write!(reader.get_mut(), "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", payload.len(), payload).unwrap();
            }
        });
        personal_rules_cli(state.path())
            .env("WENLAN_HOST", address)
            .env("WENLAN_RULESYNC", "1")
            .env(name, value)
            .args(["rules", "status"])
            .assert()
            .success();
        let headers = rx.recv_timeout(std::time::Duration::from_secs(3)).unwrap();
        assert!(headers.contains(expected), "missing {expected}: {headers}");
    }
}
