// SPDX-License-Identifier: Apache-2.0
//! `wenlan wiki refresh` end to end on a real git repository, with a stand-in
//! AI tool: no daemon, no network, no model.

#![cfg(unix)]

use assert_cmd::Command;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use wenlan_core::repo_wiki_ci::{build_manifest, check, write_manifest, REFRESH_BRANCH};

const LIB: &str = "pub fn a() {}\n\npub fn b() -> u8 {\n    1\n}\n\npub fn c() {}\n";
const MAIN: &str = "fn main() {\n    run();\n}\n\nfn run() {}\n";

fn page(title: &str, body: &str, source: &str) -> String {
    format!(
        "---\ntitle: \"{title}\"\ntype: page\norigin_id: page_{title}\nsources:\n  - {{resource: \"repo://{source}\"}}\n---\n\n# {title}\n\n{body}\n\n## Code\n\n- [`placeholder`](x)\n"
    )
}

fn sh(dir: &Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A stand-in `claude` that does what the prompt asks for `pages/b.md`, and
/// also touches a file it was not asked to.
fn fake_claude(bin: &Path) {
    let script = r#"#!/bin/sh
set -e
prompt="$2"
case "$prompt" in *"wiki/pages/b.md"*) ;; *) echo "unexpected prompt" >&2; exit 3 ;; esac
# `-i.bak` works with both GNU and BSD sed.
sed -i.bak 's/`b` returns one./`b` returns two./; s/#L3-L5"/#L3-L4"/' wiki/pages/b.md
rm wiki/pages/b.md.bak
echo "oops" >> src/main.rs
echo "stray" > notes.txt
"#;
    let path = bin.join("claude");
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn refresh_moves_lines_rewrites_changed_pages_and_pushes_a_branch() {
    let dir = tempfile::tempdir().unwrap();
    let origin = dir.path().join("origin.git");
    let repo = dir.path().join("repo");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    fake_claude(&bin);
    sh(
        dir.path(),
        &[
            "init",
            "--quiet",
            "--bare",
            "-b",
            "main",
            origin.to_str().unwrap(),
        ],
    );
    sh(
        dir.path(),
        &[
            "clone",
            "--quiet",
            origin.to_str().unwrap(),
            repo.to_str().unwrap(),
        ],
    );
    sh(&repo, &["config", "user.email", "t@example.com"]);
    sh(&repo, &["config", "user.name", "T"]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::create_dir_all(repo.join("wiki/pages")).unwrap();
    std::fs::write(repo.join("src/lib.rs"), LIB).unwrap();
    std::fs::write(repo.join("src/main.rs"), MAIN).unwrap();
    std::fs::write(
        repo.join("wiki/pages/b.md"),
        page("b", "`b` returns one.", "src/lib.rs#L3-L5"),
    )
    .unwrap();
    std::fs::write(
        repo.join("wiki/pages/main.md"),
        page("main", "`main` calls `run`.", "src/main.rs#L1-L3"),
    )
    .unwrap();
    let manifest = build_manifest(
        None,
        [
            (
                "pages/b.md".to_owned(),
                "page_b".to_owned(),
                vec![("src/lib.rs".to_owned(), 3, 5)],
            ),
            (
                "pages/main.md".to_owned(),
                "page_main".to_owned(),
                vec![("src/main.rs".to_owned(), 1, 3)],
            ),
        ],
        |path| match path {
            "src/lib.rs" => Some(LIB),
            "src/main.rs" => Some(MAIN),
            _ => None,
        },
    );
    write_manifest(&repo.join("wiki"), &manifest).unwrap();
    sh(&repo, &["add", "-A"]);
    sh(&repo, &["commit", "--quiet", "-m", "wiki"]);
    sh(&repo, &["push", "--quiet", "origin", "main"]);

    // `b` changed; `main` only moved down two lines.
    std::fs::write(repo.join("src/lib.rs"), LIB.replace("    1\n", "    2\n")).unwrap();
    std::fs::write(repo.join("src/main.rs"), format!("// one\n// two\n{MAIN}")).unwrap();
    sh(&repo, &["commit", "--quiet", "-am", "code"]);
    sh(&repo, &["push", "--quiet", "origin", "main"]);

    let path = format!("{}:/usr/bin:/bin", bin.display());
    let output = Command::cargo_bin("wenlan")
        .unwrap()
        .current_dir(&repo)
        .env("PATH", path)
        .env("HOME", dir.path())
        .env("WENLAN_NO_AUTOSTART", "1")
        .env("ANTHROPIC_API_KEY", "")
        .args(["--format", "json", "wiki", "refresh", "--pull-request"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["moved"], serde_json::json!(["pages/main.md"]));
    assert_eq!(report["rewritten"], serde_json::json!(["pages/b.md"]));
    assert_eq!(report["left"], serde_json::json!({}));
    assert_eq!(report["pull_request"], format!("pushed {REFRESH_BRANCH}"));

    // The branch holds both pages, right for the code, and nothing else.
    let changed = sh(&origin, &["diff", "--name-only", "main", REFRESH_BRANCH]);
    assert_eq!(
        changed.lines().collect::<Vec<_>>(),
        [
            "wiki/.wenlan-code.json",
            "wiki/pages/b.md",
            "wiki/pages/main.md"
        ]
    );
    let b = sh(
        &origin,
        &["show", &format!("{REFRESH_BRANCH}:wiki/pages/b.md")],
    );
    assert!(b.contains("`b` returns two."), "{b}");
    assert!(
        b.contains("- [`src/lib.rs` lines 3-4](../../src/lib.rs#L3-L4)"),
        "{b}"
    );
    let main = sh(
        &origin,
        &["show", &format!("{REFRESH_BRANCH}:wiki/pages/main.md")],
    );
    assert!(main.contains("repo://src/main.rs#L3-L5"), "{main}");
    assert!(!repo.join("notes.txt").exists());
    assert_eq!(
        std::fs::read_to_string(repo.join("src/main.rs")).unwrap(),
        format!("// one\n// two\n{MAIN}")
    );

    // Checked again on the branch: every page matches its code.
    let found = check(&repo, "wiki").unwrap();
    assert!(found.pages.is_empty(), "{found:?}");
    assert_eq!(found.unchanged, 2);
    assert_eq!(
        found.base.as_deref(),
        Some(sh(&repo, &["rev-parse", "main"]).as_str())
    );
}

#[test]
fn refresh_without_a_manifest_says_what_to_do() {
    let dir = tempfile::tempdir().unwrap();
    sh(dir.path(), &["init", "--quiet"]);
    Command::cargo_bin("wenlan")
        .unwrap()
        .current_dir(dir.path())
        .env("WENLAN_NO_AUTOSTART", "1")
        .args(["wiki", "refresh", "--agent", "none"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("wenlan wiki publish"));
}

#[test]
fn workflow_prints_the_github_actions_file() {
    Command::cargo_bin("wenlan")
        .unwrap()
        .env("WENLAN_NO_AUTOSTART", "1")
        .args(["wiki", "workflow", "--agent", "codex", "--utc-hour", "6"])
        .assert()
        .success()
        .stdout(predicates::str::contains("cron: \"17 6 * * *\""))
        .stdout(predicates::str::contains(
            "wenlan wiki refresh --folder wiki --agent codex --pull-request",
        ));
}
