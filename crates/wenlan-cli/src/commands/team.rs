// SPDX-License-Identifier: Apache-2.0
//! `wenlan team status/add/update/remove/new-key/host/join` — run a team
//! server (members, hosted Spaces) or join one with a join code.

use anyhow::{bail, Result};
use clap::Subcommand;
use wenlan_types::shared_space::{SharedHost, SharedSyncRequest};
use wenlan_types::team::{
    TeamAccess, TeamAddMemberRequest, TeamMemberKey, TeamSpaceAccess, TeamUpdateMemberRequest,
};

use crate::client::WenlanClient;
use crate::output::ResolvedFormat;

#[derive(Subcommand)]
pub enum TeamCmd {
    /// Show the members, the Spaces this computer hosts for the team, and
    /// whether the team listener is on.
    Status,
    /// Add a member and print their join code (or key) once.
    Add {
        /// The member's name.
        name: String,
        /// A Space the member can use, `Space` for read and write or
        /// `Space:read` for read only. Repeat for more Spaces.
        #[arg(long = "access", value_name = "SPACE[:read]", value_parser = parse_space_access)]
        spaces: Vec<TeamSpaceAccess>,
        /// The address the member's computer reaches this server at, put in
        /// the join code. Left out, only the key is printed.
        #[arg(long)]
        url: Option<String>,
    },
    /// Rename a member or replace the Spaces they can use.
    Update {
        /// The member's id (see `wenlan team status`).
        member_id: String,
        /// The new name.
        #[arg(long)]
        name: Option<String>,
        /// A Space the member can use, `Space` or `Space:read`. Repeat for
        /// more; the list replaces the current one.
        #[arg(long = "access", value_name = "SPACE[:read]", value_parser = parse_space_access)]
        spaces: Vec<TeamSpaceAccess>,
    },
    /// Remove a member: their key stops working at once.
    Remove {
        /// The member's id (see `wenlan team status`).
        member_id: String,
    },
    /// Give a member a new key; the old one stops working.
    NewKey {
        /// The member's id (see `wenlan team status`).
        member_id: String,
        /// The address the member's computer reaches this server at, put in
        /// the join code.
        #[arg(long)]
        url: Option<String>,
    },
    /// Host a Space of this computer for the team.
    Host {
        /// Space name.
        #[arg(value_name = "SPACE")]
        space_name: String,
    },
    /// Join a team server with a join code: list the Spaces you can use, or
    /// sync one of them with --server-space.
    Join {
        /// The join code you were sent (`https://…#wlt_…`).
        code: String,
        /// Sync this Space of the server (not `--space`, which picks a Space on this computer). Left out, the Spaces you can use
        /// are listed.
        #[arg(long = "server-space", value_name = "SERVER_SPACE")]
        server_space: Option<String>,
        /// The Space's name on this computer. Left out, the server's name.
        #[arg(long = "as", value_name = "LOCAL_SPACE", requires = "server_space")]
        local_space: Option<String>,
    },
}

/// `Space` is read and write; `Space:read` is read only (`:write` is
/// accepted too).
fn parse_space_access(raw: &str) -> Result<TeamSpaceAccess, String> {
    let (space, access) = match raw.rsplit_once(':') {
        Some((space, "read")) => (space, TeamAccess::Read),
        Some((space, "write")) => (space, TeamAccess::Write),
        _ => (raw, TeamAccess::Write),
    };
    let space = space.trim();
    if space.is_empty() {
        return Err("the Space name is empty".into());
    }
    Ok(TeamSpaceAccess {
        space: space.to_string(),
        access,
    })
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    cmd: TeamCmd,
) -> Result<()> {
    match cmd {
        TeamCmd::Status => status(client, format, quiet).await,
        TeamCmd::Add { name, spaces, url } => {
            let issued = client
                .team_add_member(&TeamAddMemberRequest { name, spaces, url })
                .await?;
            print_key(format, quiet, "Added", &issued)
        }
        TeamCmd::Update {
            member_id,
            name,
            spaces,
        } => {
            if name.is_none() && spaces.is_empty() {
                bail!("nothing to change: give --name or at least one --access");
            }
            let member = client
                .team_update_member(
                    &member_id,
                    &TeamUpdateMemberRequest {
                        name,
                        spaces: (!spaces.is_empty()).then_some(spaces),
                    },
                )
                .await?;
            output(format, quiet, &member, || {
                println!("Updated {}: {}.", member.name, access_list(&member.spaces));
            })
        }
        TeamCmd::Remove { member_id } => {
            let member = client.team_remove_member(&member_id).await?;
            output(format, quiet, &member, || {
                println!(
                    "Removed {}. Their key stopped working. Copies already on their computer stay there.",
                    member.name
                );
            })
        }
        TeamCmd::NewKey { member_id, url } => {
            let issued = client
                .team_new_member_key(&member_id, url.as_deref())
                .await?;
            print_key(format, quiet, "New key for", &issued)
        }
        TeamCmd::Host { space_name } => {
            let counts = client.team_host_space(&space_name).await?;
            output(format, quiet, &counts, || {
                println!(
                    "Hosting '{}' for the team: {} written, {} taken, {} edited at the same time, {} skipped.",
                    space_name, counts.exported, counts.applied, counts.conflicts, counts.skipped
                );
            })
        }
        TeamCmd::Join {
            code,
            server_space,
            local_space,
        } => join(client, format, quiet, &code, server_space, local_space).await,
    }
}

fn output<T: serde::Serialize>(
    format: ResolvedFormat,
    quiet: bool,
    value: &T,
    table: impl FnOnce(),
) -> Result<()> {
    if quiet {
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => crate::output::print_json(value)?,
        ResolvedFormat::Table => table(),
    }
    Ok(())
}

fn access_list(spaces: &[TeamSpaceAccess]) -> String {
    if spaces.is_empty() {
        return "no Spaces".to_string();
    }
    spaces
        .iter()
        .map(|s| match s.access {
            TeamAccess::Read => format!("{} (read)", s.space),
            TeamAccess::Write => s.space.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

async fn status(client: &WenlanClient, format: ResolvedFormat, quiet: bool) -> Result<()> {
    let status = client.team_status().await?;
    output(format, quiet, &status, || {
        match &status.listening {
            Some(addr) => println!("Team listener: on ({addr})"),
            None => println!("Team listener: off (set WENLAN_TEAM_ADDR to turn it on)"),
        }
        if status.spaces.is_empty() {
            println!("Hosted Spaces: none");
        } else {
            println!("Hosted Spaces: {}", status.spaces.join(", "));
        }
        if status.members.is_empty() {
            println!("(no members)");
            return;
        }
        println!("{:<34} {:<20} {:<9} SPACES", "ID", "NAME", "STATE");
        for m in &status.members {
            println!(
                "{:<34} {:<20} {:<9} {}",
                m.id,
                m.name,
                if m.removed_at.is_some() {
                    "removed"
                } else {
                    "active"
                },
                access_list(&m.spaces)
            );
        }
    })
}

/// Prints the key or join code. It is shown only this once.
fn print_key(
    format: ResolvedFormat,
    quiet: bool,
    verb: &str,
    issued: &TeamMemberKey,
) -> Result<()> {
    // The key is the point of the command, so --quiet still prints it as
    // the one thing on stdout, never a status line.
    if quiet {
        println!("{}", issued.join_code.as_deref().unwrap_or(&issued.key));
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => crate::output::print_json(issued)?,
        ResolvedFormat::Table => {
            println!("{} {} ({}).", verb, issued.member.name, issued.member.id);
            match &issued.join_code {
                Some(code) => println!("Join code: {code}"),
                None => println!("Key: {}", issued.key),
            }
            println!("This is shown only once. Send it to them privately.");
        }
    }
    Ok(())
}

async fn join(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    code: &str,
    server_space: Option<String>,
    local_space: Option<String>,
) -> Result<()> {
    let Some(server_space) = server_space else {
        let joined = client.team_join(code).await?;
        return output(format, quiet, &joined, || {
            println!("Server: {}", joined.url);
            println!("You are {}.", joined.me.name);
            if joined.me.spaces.is_empty() {
                println!("No Spaces are open to you yet.");
            } else {
                println!("Spaces you can use:");
                for s in &joined.me.spaces {
                    let access = match s.access {
                        TeamAccess::Read => "read only",
                        TeamAccess::Write => "read and write",
                    };
                    println!("  {} ({access})", s.space);
                }
                println!(
                    "Join one with: wenlan team join <code> --server-space <SPACE> [--as <LOCAL SPACE>]"
                );
            }
        });
    };
    let local = local_space.unwrap_or_else(|| server_space.clone());
    let counts = client
        .shared_sync(
            &local,
            &SharedSyncRequest {
                folder: String::new(),
                member_name: None,
                host: Some(SharedHost {
                    code: code.to_string(),
                    key: None,
                    space: Some(server_space.clone()),
                }),
                member_id: None,
            },
        )
        .await?;
    output(format, quiet, &counts, || {
        println!(
            "Synced '{}' with '{}' on the team server: {} sent, {} taken, {} edited at the same time, {} skipped.",
            local, server_space, counts.exported, counts.applied, counts.conflicts, counts.skipped
        );
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_access_suffix() {
        let r = parse_space_access("Work:read").unwrap();
        assert_eq!((r.space.as_str(), r.access), ("Work", TeamAccess::Read));
        let w = parse_space_access("Work").unwrap();
        assert_eq!(w.access, TeamAccess::Write);
        // A colon that is not an access suffix stays in the name.
        assert_eq!(parse_space_access("a:b").unwrap().space, "a:b");
        assert!(parse_space_access(":read").is_err());
    }
}
