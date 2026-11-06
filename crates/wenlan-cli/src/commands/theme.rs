// SPDX-License-Identifier: Apache-2.0
//! `wenlan theme list/path/check/apply/reset` — custom app themes.
//!
//! A theme is a folder under the themes folder (`wenlan theme path`) holding
//! `theme.json` and any images it names. The daemon checks every theme, so
//! this command and the app always agree on whether a theme can be used.

use anyhow::{bail, Result};
use clap::Subcommand;
use wenlan_types::theme::ThemeProblem;

use crate::client::WenlanClient;
use crate::output::ResolvedFormat;

#[derive(Subcommand)]
pub enum ThemeCommand {
    /// List custom themes and whether each one passes its checks.
    List,
    /// Print the folder where theme folders go.
    Path,
    /// Check one theme without applying it.
    Check {
        /// Theme id: its folder name, e.g. night-bunny.
        id: String,
    },
    /// Use a theme in the app. Refused when the theme fails its checks.
    Apply {
        /// Theme id: its folder name, e.g. night-bunny.
        id: String,
    },
    /// Go back to Wenlan's built-in light and dark themes.
    Reset,
}

fn print_problems(problems: &[ThemeProblem]) {
    for problem in problems {
        eprintln!("  {}: {}", problem.field, problem.message);
    }
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    command: ThemeCommand,
) -> Result<()> {
    match command {
        ThemeCommand::List => {
            let list = client.list_themes().await?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => crate::output::print_json(&list)?,
                ResolvedFormat::Table => {
                    if list.themes.is_empty() {
                        println!(
                            "No custom themes yet. Theme folders go in {}",
                            list.themes_dir
                        );
                        return Ok(());
                    }
                    for theme in &list.themes {
                        let active = list.active.as_deref() == Some(theme.id.as_str());
                        let name = theme.name.as_deref().unwrap_or("(unreadable)");
                        let state = if !theme.problems.is_empty() {
                            "needs fixes"
                        } else if active {
                            "in use"
                        } else {
                            "ready"
                        };
                        println!("{:<24} {:<28} {}", theme.id, name, state);
                    }
                }
            }
        }
        ThemeCommand::Path => {
            let list = client.list_themes().await?;
            if quiet {
                return Ok(());
            }
            match format {
                ResolvedFormat::Json => crate::output::print_json(&serde_json::json!({
                    "themes_dir": list.themes_dir,
                }))?,
                ResolvedFormat::Table => println!("{}", list.themes_dir),
            }
        }
        ThemeCommand::Check { id } => {
            let result = client.check_theme(&id).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => crate::output::print_json(&result)?,
                    ResolvedFormat::Table if result.ok => println!("{id} passes every check."),
                    ResolvedFormat::Table => {
                        eprintln!("{id} needs fixes:");
                        print_problems(&result.problems);
                    }
                }
            }
            if !result.ok {
                bail!("theme {id} failed its checks");
            }
        }
        ThemeCommand::Apply { id } => {
            let check = client.check_theme(&id).await?;
            if !check.ok {
                if !quiet {
                    eprintln!("{id} needs fixes before it can be used:");
                    print_problems(&check.problems);
                }
                bail!("theme {id} failed its checks");
            }
            let active = client.set_active_theme(Some(id.clone())).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => crate::output::print_json(&active)?,
                    ResolvedFormat::Table => println!(
                        "Using {}. The app picks it up within a few seconds.",
                        active.name.as_deref().unwrap_or(&id)
                    ),
                }
            }
        }
        ThemeCommand::Reset => {
            let active = client.set_active_theme(None).await?;
            if !quiet {
                match format {
                    ResolvedFormat::Json => crate::output::print_json(&active)?,
                    ResolvedFormat::Table => println!("Back to Wenlan's built-in themes."),
                }
            }
        }
    }
    Ok(())
}
