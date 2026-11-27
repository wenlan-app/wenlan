//! The real-renderer CI lane must run every portable test that reads
//! `WENLAN_TEST_RULESYNC_BINARY`.
//!
//! Those tests are `#[ignore]`d, so the ordinary suites never run them and a
//! new one that is missing from `scripts/rulesync-real-renderer-tests.sh` would
//! never run anywhere. The script also runs this guard, so a change that only
//! touches `wenlan-server` is still checked.

use std::collections::BTreeSet;
use std::path::Path;

use super::repo_root;

const SCRIPT: &str = "scripts/rulesync-real-renderer-tests.sh";

/// Ignored real-binary tests that CI deliberately leaves to a person.
const LOCAL_ONLY: &[(&str, &str)] = &[(
    "rulesync_routes::tests::rulesync_copied_home_adopts_and_restores_with_pinned_binary",
    "needs WENLAN_TEST_RULESYNC_COPIED_HOME, a disposable copy of a real tool home",
)];

const BINARY_VAR: &str = "\"WENLAN_TEST_RULESYNC_BINARY\"";

/// The name a `fn` line declares, whatever its visibility or qualifiers.
fn fn_name(line: &str) -> Option<String> {
    let (_, rest) = line.split_once("fn ")?;
    let before = &line[..line.len() - rest.len() - "fn ".len()];
    let qualifiers = before.split_whitespace().all(|word| {
        [
            "pub",
            "pub(crate)",
            "pub(super)",
            "async",
            "const",
            "unsafe",
        ]
        .contains(&word)
    });
    let name: String = rest
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_')
        .collect();
    (qualifiers && !name.is_empty() && rest[name.len()..].starts_with(['(', '<'])).then_some(name)
}

/// `module::tests::name` for every `#[ignore]`d test under `src` whose body
/// reads `WENLAN_TEST_RULESYNC_BINARY`, leaving out macOS-only tests.
fn real_renderer_tests(src: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut files = vec![src.to_path_buf()];
    while let Some(path) = files.pop() {
        if path.is_dir() {
            for entry in std::fs::read_dir(&path).unwrap() {
                files.push(entry.unwrap().path());
            }
            continue;
        }
        if path.extension().is_none_or(|ext| ext != "rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        if !text.contains(BINARY_VAR) {
            continue;
        }
        let module = path
            .strip_prefix(src)
            .unwrap()
            .with_extension("")
            .to_string_lossy()
            .replace(['/', '\\'], "::");
        let lines: Vec<&str> = text.lines().collect();
        for (at, line) in lines.iter().enumerate() {
            // An environment read (`var_os` or `var`), not a mention in a list.
            if !(line.contains(&format!("var_os({BINARY_VAR})"))
                || line.contains(&format!("var({BINARY_VAR})")))
            {
                continue;
            }
            let start = (0..at)
                .rev()
                .find(|&i| fn_name(lines[i]).is_some())
                .unwrap_or_else(|| panic!("{}:{} has no enclosing fn", path.display(), at + 1));
            let name = fn_name(lines[start]).unwrap();
            let attributes: Vec<&str> = lines[..start]
                .iter()
                .rev()
                .map(|line| line.trim())
                .take_while(|line| line.starts_with("#[") || line.starts_with("//"))
                .collect();
            let test = attributes.contains(&"#[test]");
            let ignored = attributes.iter().any(|line| line.starts_with("#[ignore"));
            let macos_only = attributes.contains(&"#[cfg(target_os = \"macos\")]");
            assert!(
                test && ignored,
                "{module}::{name} reads WENLAN_TEST_RULESYNC_BINARY but is not an #[ignore]d \
                 #[test]; read it in the test itself so CI can list it"
            );
            if !macos_only {
                found.insert(format!("{module}::tests::{name}"));
            }
        }
    }
    found
}

/// The names in one `NAME=( … )` bash array of the script.
fn script_list(script: &str, array: &str) -> BTreeSet<String> {
    let body = script
        .split(&format!("\n{array}=(\n"))
        .nth(1)
        .unwrap_or_else(|| panic!("{SCRIPT} has no {array} list"))
        .split("\n)")
        .next()
        .unwrap();
    body.split_whitespace().map(str::to_string).collect()
}

#[test]
fn ci_runs_every_portable_real_renderer_test() {
    let root = repo_root();
    let script = std::fs::read_to_string(root.join(SCRIPT)).unwrap();
    for (crate_dir, array) in [
        ("crates/wenlan-core", "CORE_TESTS"),
        ("crates/wenlan-server", "SERVER_TESTS"),
    ] {
        let mut expected = real_renderer_tests(&root.join(crate_dir).join("src"));
        for (name, _why) in LOCAL_ONLY {
            expected.remove(*name);
        }
        assert!(
            !expected.is_empty(),
            "{crate_dir} has no real-renderer tests"
        );
        assert_eq!(
            script_list(&script, array),
            expected,
            "{SCRIPT} {array} must list exactly the ignored {crate_dir} tests that read \
             WENLAN_TEST_RULESYNC_BINARY (add a test there, or to LOCAL_ONLY with a reason)"
        );
    }
}

#[test]
fn ci_routes_rulesync_changes_to_the_real_renderer_lane() {
    let ci = std::fs::read_to_string(repo_root().join(".github/workflows/ci.yml")).unwrap();
    let job = ci
        .split("\n  rulesync-real-renderer:\n")
        .nth(1)
        .expect("ci.yml has a rulesync-real-renderer job")
        .split("\n\n  ")
        .next()
        .unwrap();
    assert!(job.contains("runs-on: ubuntu-24.04"), "{job}");
    assert!(
        job.contains("needs.detect-changes.outputs.rulesync-real == 'true'"),
        "{job}"
    );
    assert!(job.contains(&format!("run: bash {SCRIPT}")), "{job}");
    let filter = ci
        .split("\n            rulesync-real:\n")
        .nth(1)
        .expect("ci.yml has a rulesync-real paths filter")
        .split("\n\n")
        .next()
        .unwrap();
    for pattern in [
        "'crates/wenlan-core/src/rulesync_*.rs'",
        "'crates/wenlan-server/src/rulesync_*.rs'",
        &format!("'{SCRIPT}'"),
    ] {
        assert!(
            filter.contains(pattern),
            "rulesync-real filter omits {pattern}"
        );
    }
    assert!(ci.contains("expect_job rulesync-real-renderer "));
    // Every file holding a real-renderer test must wake the filter.
    for crate_dir in ["crates/wenlan-core", "crates/wenlan-server"] {
        let src = repo_root().join(crate_dir).join("src");
        for test in real_renderer_tests(&src) {
            let first = test.split("::").next().unwrap();
            assert!(
                first.starts_with("rulesync_"),
                "{test} lives outside the rulesync_* files the filter watches"
            );
        }
    }
}
