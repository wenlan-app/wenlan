// SPDX-License-Identifier: Apache-2.0
use super::*;

fn daily(enabled: bool, runs_on: RepoWikiRunner) -> RepoWikiDaily {
    RepoWikiDaily {
        enabled,
        hour: 9,
        agent: RepoWikiAgent::Claude,
        runs_on,
    }
}

#[test]
fn the_local_hour_becomes_utc() {
    assert_eq!(utc_hour(9, 8 * 3600), 1);
    assert_eq!(utc_hour(1, 8 * 3600), 17);
    assert_eq!(utc_hour(9, -7 * 3600), 16);
    assert_eq!(utc_hour(23, -7 * 3600), 6);
    assert_eq!(utc_hour(9, 5 * 3600 + 1800), 3);
    assert_eq!(utc_hour(9, 0), 9);
}

#[test]
fn the_workflow_is_written_only_while_the_daily_update_runs_on_github() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(WORKFLOW_PATH);
    let github = daily(true, RepoWikiRunner::Github);

    maintain_workflow(dir.path(), "wiki", Some(&github), 8 * 3600).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.starts_with(WORKFLOW_MARK), "{text}");
    assert!(text.contains("cron: \"17 1 * * *\""), "{text}");

    // Back on this computer, or off: Wenlan's file goes.
    maintain_workflow(
        dir.path(),
        "wiki",
        Some(&daily(true, RepoWikiRunner::Computer)),
        0,
    )
    .unwrap();
    assert!(!path.exists());
    maintain_workflow(dir.path(), "wiki", Some(&github), 0).unwrap();
    maintain_workflow(
        dir.path(),
        "wiki",
        Some(&daily(false, RepoWikiRunner::Github)),
        0,
    )
    .unwrap();
    assert!(!path.exists());

    // A file of the user's at that path stays.
    std::fs::write(&path, "name: mine\n").unwrap();
    maintain_workflow(dir.path(), "wiki", None, 0).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "name: mine\n");
}

#[test]
fn the_token_is_read_whole_through_terminal_codes() {
    let printed = "\x1b[2K\x1b[1G✓ Long-lived authentication token created successfully!\r\n\r\n\
                   Your OAuth token (valid for 1 year):\r\n\r\n\x1b[1msk-ant-oat01-Abc_def-123\x1b[22m\r\n\
                   Store this token securely.";
    assert_eq!(
        token_in(printed).as_deref(),
        Some("sk-ant-oat01-Abc_def-123")
    );
    // Still being written: not taken yet.
    assert_eq!(token_in("Your token: sk-ant-oat01-Abc"), None);
    assert_eq!(token_in("Opening the browser..."), None);
}
