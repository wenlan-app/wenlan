// SPDX-License-Identifier: Apache-2.0
//! `wenlan skills` against a scripted stand-in for the daemon.

mod common;

use assert_cmd::Command;
use common::spawn_stub;
use predicates::prelude::*;
use serde_json::json;

fn cli(base: &str) -> Command {
    let mut cmd = Command::cargo_bin("wenlan").unwrap();
    cmd.env("WENLAN_HOST", base)
        .env("WENLAN_NO_AUTOSTART", "1")
        .env_remove("WENLAN_AGENT_NAME")
        .env_remove("WENLAN_SPACE")
        .env_remove("WENLAN_DEFAULT_SPACE");
    cmd
}

fn held() -> String {
    json!({"skills": [{
        "name": "meeting-notes", "source": "o/r", "skillId": "meeting-notes",
        "repoPath": "skills/meeting-notes", "commit": "c", "hash": "h", "verdict": "pass",
        "scope": {"kind": "all"}, "description": "Turn transcripts into decisions.",
        "installedAt": 1, "updatedAt": 1, "additions": "Use our template.",
    }]})
    .to_string()
}

/// UX-SKILL-12: `wenlan skills` and `wenlan skills list` read the same list
/// an AI tool gets from `list_skills`; piped, every field is kept.
#[test]
fn ux_skill_12_skills_list_reads_the_held_skills() {
    for args in [vec!["skills"], vec!["skills", "list"]] {
        let (base, received) = spawn_stub(vec![("GET /api/skills/registry/installed", held())]);
        cli(&base)
            .args(["--format", "table"])
            .args(&args)
            .assert()
            .success()
            .stdout(
                "Your skills\n\n\
                 - meeting-notes  All Spaces, from o/r, safety checks passed\n  \
                 Turn transcripts into decisions.\n  with your additions\n",
            );
        assert_eq!(
            received.try_recv().unwrap().0,
            "GET /api/skills/registry/installed"
        );
    }
    let (base, _received) = spawn_stub(vec![("GET /api/skills/registry/installed", held())]);
    cli(&base)
        .args(["skills", "list"])
        .assert()
        .success()
        .stdout(predicate::str::contains("\"skillId\": \"meeting-notes\""))
        .stdout(predicate::str::contains(
            "\"additions\": \"Use our template.\"",
        ));
}

/// UX-SKILL-12: `wenlan skills search` (and its other name, `find`) asks
/// the daemon with the words encoded and prints one line per skill.
#[test]
fn ux_skill_12_skills_search_and_find_ask_the_same_question() {
    let found = json!({"via": "skills.sh", "rows": [
        {"source": "o/r", "skillId": "meeting-notes", "name": "meeting-notes", "installs": 12, "verdict": "pass"},
        {"source": "o/r", "skillId": "x", "name": "x", "installs": 1, "verdict": "warn"},
    ]})
    .to_string();
    for verb in ["search", "find"] {
        let (base, received) = spawn_stub(vec![(
            "GET /api/skills/registry/search?q=meeting+notes+%26+more",
            found.clone(),
        )]);
        cli(&base)
            .args([
                "--format",
                "table",
                "skills",
                verb,
                "meeting notes",
                "& more",
            ])
            .assert()
            .success()
            .stdout(
                "Skills for \"meeting notes & more\"\n\n  \
                 o/r/meeting-notes  12 installs, safety checks passed\n\n\
                 1 with warnings not shown. Add --warnings to see them.\n\n\
                 Look at one before installing: wenlan skills show <owner/repo/skill>\n",
            );
        assert!(received.try_recv().is_ok());
    }
}

/// UX-SKILL-12: `wenlan skills show` (and `preview`) prints the whole skill.
#[test]
fn ux_skill_12_skills_show_prints_the_files_before_any_install() {
    let preview = json!({
        "source": "o/r", "skillId": "meeting-notes",
        "commit": "0123456789abcdef0123456789abcdef01234567", "repoPath": "skills/meeting-notes",
        "name": "Meeting Notes", "description": "Turn transcripts into decisions.",
        "files": [{"path": "SKILL.md", "text": "# Notes\n"}, {"path": "logo.png", "text": null}],
        "runnable": [], "safety": {"verdict": "pass", "audits": [
            {"provider": "Snyk", "status": "pass", "summary": "ok"}]},
        "sourceUrl": "https://github.com/o/r/tree/0123/skills/meeting-notes",
        "skillsShUrl": "https://skills.sh/o/r/meeting-notes", "installedAs": null,
    })
    .to_string();
    for verb in ["show", "preview"] {
        let (base, _received) = spawn_stub(vec![(
            "GET /api/skills/registry/preview?source=o%2Fr&skillId=meeting-notes",
            preview.clone(),
        )]);
        cli(&base)
            .args(["--format", "table", "skills", verb, "o/r/meeting-notes"])
            .assert()
            .success()
            .stdout(predicate::str::starts_with(
                "Meeting Notes  (o/r/meeting-notes)\nTurn transcripts into decisions.\n\n\
                 Safety: passed 1 safety check\n  Snyk: pass (ok)\nText only, no programs.\n",
            ))
            .stdout(predicate::str::contains(
                "\n----- SKILL.md -----\n# Notes\n\n----- logo.png -----\n(not a text file)\n----- end -----\n",
            ));
    }
    // A name that is not owner/repo/skill never reaches the daemon.
    cli("http://127.0.0.1:9")
        .args(["skills", "show", "meeting-notes"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("owner/repo/skill"));
}

#[test]
fn skills_scan_lists_what_the_tools_already_have() {
    let found = json!({"skills": [
        {"name": "my-review", "path": "/home/me/.claude/skills/my-review", "description": null},
    ]})
    .to_string();
    let (base, _received) = spawn_stub(vec![("GET /api/skills/registry/local", found)]);
    cli(&base)
        .args(["--format", "table", "skills", "scan"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with(
            "Already on this computer\n\n- my-review  /home/me/.claude/skills/my-review\n",
        ));
}

/// UX-SKILL-13: without the person at a terminal (a test, a pipe, a script,
/// an AI tool's shell) install and import stop before the daemon is asked
/// anything, and no flag answers for the person.
#[test]
fn ux_skill_13_install_import_and_remove_refuse_without_the_person_at_a_terminal() {
    // No route is served: any request at all would fail the stand-in.
    let (base, received) = spawn_stub(Vec::new());
    for args in [
        vec!["skills", "install", "o/r/meeting-notes"],
        vec!["skills", "import"],
        vec!["skills", "import", "my-review"],
        vec!["skills", "remove", "meeting-notes"],
    ] {
        cli(&base)
            .args(&args)
            .write_stdin("yes\nyes\n")
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "only the person can install, take in or remove a skill",
            ));
        cli(&base)
            .env("WENLAN_AGENT_NAME", "claude-code")
            .args(&args)
            .assert()
            .failure()
            .stderr(predicate::str::contains("running as an AI tool"));
    }
    assert!(received.try_recv().is_err(), "the daemon was asked nothing");
    for flag in ["--yes", "-y", "--force"] {
        for (action, skill) in [
            ("install", "o/r/meeting-notes"),
            ("remove", "meeting-notes"),
        ] {
            cli(&base)
                .args(["skills", action, skill, flag])
                .assert()
                .failure()
                .stderr(predicate::str::contains("unexpected argument"));
        }
    }
}
