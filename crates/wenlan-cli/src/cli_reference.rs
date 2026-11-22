// SPDX-License-Identifier: Apache-2.0
//! Renders the "Subcommands" reference in `crates/wenlan-cli/README.md` from the
//! clap definitions in `main.rs`, so the README can never describe a flag that
//! the binary does not have. The test below compares the marked region of the
//! README with the rendered text; set `WENLAN_UPDATE_CLI_REFERENCE=1` to rewrite
//! the region instead:
//!
//! ```text
//! WENLAN_UPDATE_CLI_REFERENCE=1 cargo test -p wenlan --bin wenlan cli_reference
//! ```

use clap::{Arg, ArgAction, Command};
use std::fmt::Write as _;

pub(crate) const START_MARKER: &str = "<!-- cli-reference:start -->";
pub(crate) const END_MARKER: &str = "<!-- cli-reference:end -->";
const BANNER: &str =
    "<!-- generated from the clap definitions in crates/wenlan-cli/src/main.rs by \
`WENLAN_UPDATE_CLI_REFERENCE=1 cargo test -p wenlan --bin wenlan cli_reference`; do not edit -->";

/// Render the full marked region, markers included.
pub(crate) fn render_region(root: &mut Command) -> String {
    let mut out = String::new();
    out.push_str(START_MARKER);
    out.push('\n');
    out.push_str(BANNER);
    out.push('\n');
    out.push_str(&render_body(root));
    out.push_str(END_MARKER);
    out.push('\n');
    out
}

fn render_body(root: &mut Command) -> String {
    root.build();
    let mut out = String::new();
    out.push_str("## Subcommands\n\n");
    out.push_str(
        "This section is generated from the same definitions that produce `wenlan --help` and \
`wenlan <command> --help`. Hidden aliases are not listed. These global options are parsed \
for every subcommand; commands that do not work on a Space reject `--space` and \
`--all-spaces` at runtime:\n\n",
    );
    for arg in root
        .get_arguments()
        .filter(|arg| arg.is_global_set() && !arg.is_hide_set())
    {
        out.push_str(&render_arg(arg));
    }
    out.push('\n');

    for (heading, names) in super::HELP_GROUPS {
        let _ = writeln!(out, "**{heading}**\n");
        for name in names.iter() {
            let sub = root
                .find_subcommand_mut(name)
                .unwrap_or_else(|| panic!("HELP_GROUPS names unknown subcommand {name}"));
            render_command(&mut out, sub, &format!("wenlan {name}"), 3);
        }
    }
    out
}

fn render_command(out: &mut String, cmd: &mut Command, path: &str, level: usize) {
    let hashes = "#".repeat(level.min(6));
    let _ = writeln!(out, "{hashes} `{path}`\n");
    let about = cmd
        .get_long_about()
        .or(cmd.get_about())
        .map(|text| text.to_string().trim().to_string())
        .unwrap_or_default();
    if !about.is_empty() {
        let _ = writeln!(out, "{}\n", ensure_period(&about));
    }
    let usage = cmd.render_usage().to_string();
    let usage = usage.trim_start_matches("Usage:").trim();
    let _ = writeln!(out, "```text\n{usage}\n```\n");

    let positionals: Vec<&Arg> = cmd
        .get_positionals()
        .filter(|arg| !arg.is_hide_set())
        .collect();
    if !positionals.is_empty() {
        out.push_str("Arguments:\n\n");
        for arg in positionals {
            out.push_str(&render_arg(arg));
        }
        out.push('\n');
    }
    let options: Vec<&Arg> = cmd
        .get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_global_set() && !arg.is_hide_set())
        .filter(|arg| !matches!(arg.get_id().as_str(), "help" | "version"))
        .collect();
    if !options.is_empty() {
        out.push_str("Options:\n\n");
        for arg in options {
            out.push_str(&render_arg(arg));
        }
        out.push('\n');
    }

    let children: Vec<String> = cmd
        .get_subcommands()
        // The reference describes the opted-in setup too. Live help hides
        // Status until the rulesync flag is present.
        .filter(|sub| {
            sub.get_name() != "help"
                && (!sub.is_hide_set() || (path == "wenlan rules" && sub.get_name() == "status"))
        })
        .map(|sub| sub.get_name().to_string())
        .collect();
    for child in children {
        let sub = cmd
            .find_subcommand_mut(&child)
            .expect("child subcommand listed a moment ago");
        render_command(out, sub, &format!("{path} {child}"), level + 1);
    }
}

fn render_arg(arg: &Arg) -> String {
    let mut line = format!("- `{}`", arg_signature(arg));
    let help = arg
        .get_long_help()
        .or(arg.get_help())
        .map(|text| text.to_string().trim().to_string())
        .unwrap_or_default();
    if !help.is_empty() {
        line.push_str(": ");
        line.push_str(&ensure_period(&help));
    }
    let possible: Vec<String> = arg
        .get_possible_values()
        .iter()
        .filter(|value| !value.is_hide_set())
        .map(|value| match value.get_help() {
            Some(help) => format!("`{}` ({})", value.get_name(), help.to_string().trim()),
            None => format!("`{}`", value.get_name()),
        })
        .collect();
    if !possible.is_empty() {
        let _ = write!(line, " Possible values: {}.", possible.join(", "));
    }
    let takes_default = !matches!(
        arg.get_action(),
        ArgAction::SetTrue
            | ArgAction::SetFalse
            | ArgAction::Count
            | ArgAction::Help
            | ArgAction::Version
    );
    if takes_default {
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect();
        if !defaults.is_empty() {
            let _ = write!(line, " Default: `{}`.", defaults.join(" "));
        }
    }
    line.push('\n');
    line
}

fn arg_signature(arg: &Arg) -> String {
    let value_name = || -> String {
        arg.get_value_names()
            .map(|names| {
                names
                    .iter()
                    .map(|name| format!("<{name}>"))
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_else(|| format!("<{}>", arg.get_id().as_str().to_uppercase()))
    };
    if arg.is_positional() {
        let mut name = value_name();
        let variadic = matches!(arg.get_action(), ArgAction::Append)
            || arg
                .get_num_args()
                .is_some_and(|range| range.max_values() > 1);
        if !arg.is_required_set() {
            name = format!("[{}]", name.trim_start_matches('<').trim_end_matches('>'));
        }
        if variadic {
            name.push_str("...");
        }
        return name;
    }
    let mut parts = Vec::new();
    if let Some(short) = arg.get_short() {
        parts.push(format!("-{short}"));
    }
    if let Some(long) = arg.get_long() {
        parts.push(format!("--{long}"));
    }
    let mut signature = parts.join(", ");
    if arg.get_action().takes_values() {
        signature.push(' ');
        signature.push_str(&value_name());
    }
    signature
}

fn ensure_period(text: &str) -> String {
    let trimmed = text.trim_end();
    if trimmed.ends_with(['.', '!', '?', ':']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;
    use std::path::{Path, PathBuf};

    fn readme_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md")
    }

    fn region_bounds(readme: &str) -> (usize, usize) {
        let start = readme
            .find(START_MARKER)
            .unwrap_or_else(|| panic!("{} has no {START_MARKER}", readme_path().display()));
        let end_start = readme[start..]
            .find(END_MARKER)
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("{} has no {END_MARKER}", readme_path().display()));
        (start, end_start + END_MARKER.len() + 1)
    }

    /// The README's Subcommands section must equal what the clap tree renders.
    /// `WENLAN_UPDATE_CLI_REFERENCE=1` rewrites it; CI never sets that.
    #[test]
    fn cli_reference_matches_readme() {
        let path = readme_path();
        // Compare with LF line endings so a CRLF checkout (Windows without
        // `eol=lf`) does not fail on unchanged content; a rewrite emits LF.
        let readme = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("read {}: {error}", path.display()))
            .replace("\r\n", "\n");
        let (start, end) = region_bounds(&readme);
        let expected = render_region(&mut super::super::Cli::command());
        let actual = &readme[start..end];
        if actual == expected {
            return;
        }
        if std::env::var_os("WENLAN_UPDATE_CLI_REFERENCE").is_some() {
            let updated = format!("{}{}{}", &readme[..start], expected, &readme[end..]);
            std::fs::write(&path, updated)
                .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
            return;
        }
        let first_diff = actual
            .lines()
            .zip(expected.lines())
            .position(|(a, e)| a != e)
            .map(|index| index + 1)
            .unwrap_or(actual.lines().count().min(expected.lines().count()) + 1);
        panic!(
            "{} drifted from the clap definitions (first differing line {first_diff} of the \
generated region). Regenerate with: WENLAN_UPDATE_CLI_REFERENCE=1 cargo test -p wenlan --bin \
wenlan cli_reference",
            path.display()
        );
    }

    /// Every visible top-level subcommand gets one `###` heading, in HELP_GROUPS order,
    /// and hidden subcommands never appear.
    #[test]
    fn cli_reference_lists_every_visible_subcommand() {
        let mut cmd = super::super::Cli::command();
        let body = render_body(&mut cmd);
        let headings: Vec<&str> = body
            .lines()
            .filter(|line| line.starts_with("### `wenlan "))
            .collect();
        let expected: Vec<String> = super::super::HELP_GROUPS
            .iter()
            .flat_map(|(_, names)| names.iter())
            .map(|name| format!("### `wenlan {name}`"))
            .collect();
        assert_eq!(headings, expected);
        for hidden in cmd
            .get_subcommands()
            .filter(|sub| sub.is_hide_set())
            .map(|sub| format!("`wenlan {}`", sub.get_name()))
        {
            assert!(
                !body.contains(&hidden),
                "hidden subcommand leaked: {hidden}"
            );
        }
    }
}
