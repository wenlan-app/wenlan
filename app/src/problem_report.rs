// SPDX-License-Identifier: AGPL-3.0-only
//! The text behind "Report a problem" on the screen the app shows when it
//! cannot reach its background service: the app version, the platform, and
//! the last lines of the logs that say why. Nothing is sent from here; the
//! frontend copies it and opens a prefilled issue the person reviews first.

use std::path::{Path, PathBuf};

/// Log lines kept from the end of each log file.
const TAIL_LINES: usize = 40;

/// The report as plain Markdown, ready to paste into an issue.
#[tauri::command]
pub fn problem_report() -> String {
    let home = dirs::home_dir();
    let logs: Vec<(String, String)> = log_files()
        .into_iter()
        .filter_map(|path| {
            let text = std::fs::read_to_string(&path).ok()?;
            let label = path.file_name()?.to_string_lossy().into_owned();
            Some((label, tail_lines(&text, TAIL_LINES)))
        })
        .collect();
    render_report(
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH,
        &logs,
        home.as_deref(),
    )
}

/// The app log carries the daemon's output on Windows and Linux, where the
/// app starts it as a sidecar; on macOS the daemon writes its own file.
fn log_files() -> Vec<PathBuf> {
    let mut files = Vec::new();
    if cfg!(target_os = "macos") {
        files.push(crate::identity_paths::app_data_dir().join("logs/wenlan-server.log"));
    }
    files.push(crate::app_log_dir().join(crate::app_log_file_name()));
    files
}

fn tail_lines(text: &str, count: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(count)..].join("\n")
}

fn render_report(
    version: &str,
    os: &str,
    arch: &str,
    logs: &[(String, String)],
    home: Option<&Path>,
) -> String {
    let mut report = format!(
        "Wenlan could not reach its background service.\n\n- Version: {version}\n- Platform: {os} {arch}\n"
    );
    if logs.is_empty() {
        report.push_str("\nNo log file was found.\n");
    }
    for (label, tail) in logs {
        report.push_str(&format!(
            "\n<details><summary>{label}</summary>\n\n```\n{tail}\n```\n</details>\n"
        ));
    }
    // The home folder names the person; the rest of a path is enough to debug.
    match home.map(|home| home.to_string_lossy().into_owned()) {
        Some(home) if !home.is_empty() => report.replace(&home, "~"),
        _ => report,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_last_lines() {
        assert_eq!(tail_lines("a\nb\nc\nd", 2), "c\nd");
        assert_eq!(tail_lines("a\nb", 5), "a\nb");
    }

    #[test]
    fn names_the_version_and_platform_and_hides_the_home_folder() {
        let home = Path::new("/Users/someone");
        let report = render_report(
            "0.20.6",
            "windows",
            "x86_64",
            &[(
                "wenlan.log".to_string(),
                "[daemon] cache=/Users/someone/data failed".to_string(),
            )],
            Some(home),
        );
        assert!(report.contains("- Version: 0.20.6"));
        assert!(report.contains("- Platform: windows x86_64"));
        assert!(report.contains("<summary>wenlan.log</summary>"));
        assert!(report.contains("cache=~/data failed"));
        assert!(!report.contains("someone"));
    }

    #[test]
    fn says_so_when_there_is_no_log() {
        let report = render_report("0.20.6", "linux", "x86_64", &[], None);
        assert!(report.contains("No log file was found."));
    }
}
