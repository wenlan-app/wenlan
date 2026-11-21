// SPDX-License-Identifier: Apache-2.0
//! `wenlan team start/stop/status/invite/update/remove/new-key/host/join/leave`
//! — run a team from this computer (its people, the Spaces it hosts) or
//! join one with an invite link.

use anyhow::{bail, Result};
use clap::Subcommand;
use wenlan_types::shared_space::{SharedHost, SharedSyncRequest, SharedSyncStatus};
use wenlan_types::team::{
    TeamAccess, TeamAddMemberRequest, TeamEveryone, TeamJoinResponse, TeamMember, TeamMemberKey,
    TeamMembershipInfo, TeamSpaceAccess, TeamStartRequest, TeamStatus, TeamUpdateMemberRequest,
};

use crate::client::WenlanClient;
use crate::output::ResolvedFormat;

#[derive(Subcommand)]
pub enum TeamCmd {
    /// Start a team on this computer: others reach it with an invite link.
    Start {
        /// The team's name, such as the company's. Everyone on it sees it.
        name: String,
        /// Your own name, as the team sees what you write. Left out, the
        /// name you already have in Wenlan.
        #[arg(long = "as", value_name = "YOUR NAME")]
        person: Option<String>,
        /// The address others reach this computer at, put in every invite
        /// link. Left out, Wenlan picks one: its Tailscale address first,
        /// else its address on the local network. A loopback address
        /// (`http://127.0.0.1:<PORT>`, `localhost`) keeps the team on this
        /// computer only: nothing else on the network can reach it.
        #[arg(long)]
        url: Option<String>,
        /// The port the team listens on. Left out, 7879.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Stop letting the team reach this computer. The team, its people and
    /// Spaces stay; `wenlan team start` turns it on again.
    Stop,
    /// Show the team this computer runs (its people and Spaces) and the
    /// teams it has joined.
    Status,
    /// Name the team; every member's Wenlan shows the name.
    Rename {
        /// The team's name, such as the company's.
        name: String,
    },
    /// Invite someone: add them and print their invite link once.
    #[command(alias = "add")]
    Invite {
        /// The person's name.
        name: String,
        /// A Space they can use, `Space` for read and write or
        /// `Space:read` for read only. Repeat for more Spaces. Left out,
        /// they get the Spaces open to everyone on the team.
        #[arg(long = "access", value_name = "SPACE[:read]", value_parser = parse_space_access)]
        spaces: Vec<TeamSpaceAccess>,
        /// The address their computer reaches this one at, put in the
        /// invite link. Left out, the address the team was started with.
        #[arg(long)]
        url: Option<String>,
    },
    /// Rename a member or replace the Spaces they can use.
    Update {
        /// The member's name, or their id from `wenlan team status --format json`.
        #[arg(value_name = "MEMBER")]
        member: String,
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
        /// The member's name, or their id from `wenlan team status --format json`.
        #[arg(value_name = "MEMBER")]
        member: String,
    },
    /// Give a member a new invite link; the old one stops working.
    NewKey {
        /// The member's name, or their id from `wenlan team status --format json`.
        #[arg(value_name = "MEMBER")]
        member: String,
        /// The address the member's computer reaches this one at, put in
        /// the invite link. Left out, the address the team was started with.
        #[arg(long)]
        url: Option<String>,
    },
    /// Share a Space of this computer with the team.
    #[command(name = "share", alias = "host")]
    Host {
        /// Space name.
        #[arg(value_name = "SPACE")]
        space_name: String,
        /// Open it to everyone on the team, now and when invited later:
        /// `--everyone` to read and write, `--everyone read` to read only,
        /// `--everyone nobody` to take that back. Left out, only the people
        /// given it with `--access` can use it.
        #[arg(long, value_name = "ACCESS", num_args = 0..=1, default_missing_value = "write", value_parser = parse_everyone)]
        everyone: Option<TeamEveryone>,
    },
    /// Join a team with an invite link: every Space open to you is synced
    /// to this computer, and Spaces opened to you later follow.
    Join {
        /// The invite link you were sent (`https://…/join#wlt_…`).
        #[arg(value_name = "INVITE LINK")]
        code: String,
        /// Take only this Space of the team (not `--space`, which picks a
        /// Space on this computer).
        #[arg(
            long = "server-space",
            value_name = "SERVER_SPACE",
            conflicts_with = "list"
        )]
        server_space: Option<String>,
        /// The Space's name on this computer. Left out, the team's name for it.
        #[arg(long = "as", value_name = "LOCAL_SPACE", requires = "server_space")]
        local_space: Option<String>,
        /// Only show the Spaces the link opens; join nothing.
        #[arg(long)]
        list: bool,
    },
    /// Leave a team you joined: its Spaces stop syncing here. The pages
    /// already on this computer stay.
    Leave {
        /// The team's name or its server's address. Not needed when this
        /// computer is on one team.
        #[arg(value_name = "TEAM")]
        team: Option<String>,
    },
    /// List the AI tools you connected to a team with `wenlan login`.
    Keys {
        /// The team's name or its server's address. Left out, every team
        /// this computer joined.
        #[arg(long, value_name = "TEAM")]
        team: Option<String>,
    },
    /// Remove an AI tool you connected: it stops working at its next request.
    RevokeKey {
        /// The tool's id, from `wenlan team keys`.
        id: String,
        /// The team's name or its server's address. Not needed when this
        /// computer is on one team.
        #[arg(long, value_name = "TEAM")]
        team: Option<String>,
    },
}

fn parse_everyone(raw: &str) -> Result<TeamEveryone, String> {
    match raw.trim().to_lowercase().as_str() {
        "write" => Ok(TeamEveryone::Write),
        "read" => Ok(TeamEveryone::Read),
        "nobody" => Ok(TeamEveryone::Nobody),
        _ => Err("use write, read or nobody".into()),
    }
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
        id: None,
    })
}

pub async fn run(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    cmd: TeamCmd,
) -> Result<()> {
    match cmd {
        TeamCmd::Start {
            name,
            person,
            url,
            port,
        } => start(client, format, quiet, name, person, url, port).await,
        TeamCmd::Stop => {
            let team = client.team_stop().await?;
            output(format, quiet, &team, || {
                println!("The team can no longer reach this computer. The team, its people and Spaces stay.");
                println!("Turn it on again: wenlan team start <NAME>");
            })
        }
        TeamCmd::Status => status(client, format, quiet).await,
        TeamCmd::Rename { name } => {
            let team = client.team_rename(&name).await?;
            output(format, quiet, &team, || {
                println!(
                    "The team is now called {}",
                    team.name.as_deref().unwrap_or(&name)
                );
            })
        }
        TeamCmd::Invite { name, spaces, url } => {
            invite(client, format, quiet, name, spaces, url).await
        }
        TeamCmd::Update {
            member,
            name,
            spaces,
        } => {
            if name.is_none() && spaces.is_empty() {
                bail!("nothing to change: give --name or at least one --access");
            }
            let member_id = member_id(client, &member).await?;
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
        TeamCmd::Remove { member } => {
            let member_id = member_id(client, &member).await?;
            let member = client.team_remove_member(&member_id).await?;
            output(format, quiet, &member, || {
                println!(
                    "Removed {}. Their key stopped working. Copies already on their computer stay there.",
                    member.name
                );
            })
        }
        TeamCmd::NewKey { member, url } => {
            let member_id = member_id(client, &member).await?;
            let issued = client
                .team_new_member_key(&member_id, url.as_deref())
                .await?;
            print_key(format, quiet, "New invite link for", &issued, None)
        }
        TeamCmd::Host {
            space_name,
            everyone,
        } => {
            // Hosting an already hosted Space is another sync pass, so this
            // is also how who has it changes later.
            let counts = client.team_host_space(&space_name, None).await?;
            if everyone.is_some() {
                client.team_host_space(&space_name, everyone).await?;
            }
            output(format, quiet, &counts, || {
                println!(
                    "Hosting '{}' for the team: {} written, {} taken, {} edited at the same time, {} skipped.",
                    space_name, counts.exported, counts.applied, counts.conflicts, counts.skipped
                );
                match everyone {
                    Some(TeamEveryone::Write) => {
                        println!("Everyone on the team can read and change it.")
                    }
                    Some(TeamEveryone::Read) => println!("Everyone on the team can read it."),
                    Some(TeamEveryone::Nobody) => println!("Only the people given it can use it."),
                    None => {}
                }
            })
        }
        TeamCmd::Join {
            code,
            server_space,
            local_space,
            list,
        } => match (server_space, list) {
            (Some(server_space), _) => {
                join_one(client, format, quiet, &code, server_space, local_space).await
            }
            (None, true) => join_list(client, format, quiet, &code).await,
            (None, false) => join_all(client, format, quiet, &code).await,
        },
        TeamCmd::Leave { team } => leave(client, format, quiet, team).await,
        TeamCmd::Keys { team } => tool_keys(client, format, quiet, team).await,
        TeamCmd::RevokeKey { id, team } => revoke_tool_key(client, format, quiet, id, team).await,
    }
}

/// The server a `--team` names, when one is named.
async fn named_team(client: &WenlanClient, given: Option<&str>) -> Result<Option<String>> {
    let Some(given) = given else { return Ok(None) };
    let (joined, _) = joined(client).await?;
    Ok(Some(pick_team(&joined, Some(given))?.url.clone()))
}

fn print_tools(on: &wenlan_types::team::TeamToolsOnServer) {
    let label = match on.team.as_ref().and_then(|info| info.name.as_deref()) {
        Some(name) => format!("{name} ({})", on.url),
        None => on.url.clone(),
    };
    if let Some(error) = &on.error {
        println!("{label}: {error}");
        return;
    }
    if on.tools.is_empty() {
        println!(
            "{label}: no AI tools. Connect one with `wenlan login {}`.",
            on.url
        );
        return;
    }
    println!("{label}:");
    for tool in &on.tools {
        let day = tool.created_at.get(..10).unwrap_or(&tool.created_at);
        println!("  {}  {}  added {day}", tool.id, tool.name);
    }
}

async fn tool_keys(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    team: Option<String>,
) -> Result<()> {
    let url = named_team(client, team.as_deref()).await?;
    let found = client.team_my_tools(url.as_deref()).await?;
    output(format, quiet, &found, || found.iter().for_each(print_tools))
}

async fn revoke_tool_key(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    id: String,
    team: Option<String>,
) -> Result<()> {
    let url = named_team(client, team.as_deref()).await?;
    let left = client.team_remove_my_tool(&id, url.as_deref()).await?;
    output(format, quiet, &left, || {
        println!("Removed. It stops working at its next request.");
        print_tools(&left);
    })
}

async fn start(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    name: String,
    person: Option<String>,
    url: Option<String>,
    port: Option<u16>,
) -> Result<()> {
    let asked_as = person.is_some();
    let started = client
        .team_start(&TeamStartRequest {
            name: name.clone(),
            // Empty: the daemon uses the name this person already has.
            person: person.unwrap_or_default(),
            url,
            port,
        })
        .await;
    let team = match started {
        Ok(team) => team,
        // The daemon's own sentence for a missing name, plus the flag that
        // fixes it from here.
        Err(error) if !asked_as && error.to_string().contains("your name") => {
            bail!("Wenlan does not know your name yet. Add it: wenlan team start \"{name}\" --as \"<YOUR NAME>\"")
        }
        Err(error) => return Err(error),
    };
    // Best effort: an older daemon, or one off Windows, has nothing to say.
    let firewall = client.team_firewall().await.ok().flatten();
    output(format, quiet, &team, || {
        let shown = team
            .team
            .as_ref()
            .and_then(|info| info.name.clone())
            .unwrap_or(name);
        println!("Started the team {shown} on this computer.");
        match &team.url {
            Some(url) => println!("Invite links use: {url}"),
            None => println!(
                "No address others can reach was found. Give one: wenlan team start \"{shown}\" --url <ADDRESS>"
            ),
        }
        if let Some(addr) = &team.listening {
            println!("Listening on: {addr}");
        }
        if firewall.is_some_and(|firewall| !firewall.open) {
            println!("Windows Firewall is keeping other computers out. In the Wenlan app, open Settings › Team and choose Allow.");
        }
        println!("Next: wenlan team share <SPACE> --everyone, then wenlan team invite <NAME>");
    })
}

/// A team member's id from what was typed: their id, or their name when
/// exactly one member still on the team has it. Never a silent pick.
async fn member_id(client: &WenlanClient, given: &str) -> Result<String> {
    let status = client.team_status().await?;
    resolve_member(&status.members, given).map(|member| member.id.clone())
}

fn resolve_member<'a>(members: &'a [TeamMember], given: &str) -> Result<&'a TeamMember> {
    let given = given.trim();
    if let Some(member) = members.iter().find(|member| member.id == given) {
        return Ok(member);
    }
    let named: Vec<&TeamMember> = members
        .iter()
        .filter(|member| member.removed_at.is_none() && same_name(&member.name, given))
        .collect();
    match named.as_slice() {
        [member] => Ok(member),
        [] => bail!("no one on the team is called \"{given}\". See who is: wenlan team status"),
        several => {
            let ids = several
                .iter()
                .map(|member| format!("  {}  {}", member.id, access_list(&member.spaces)))
                .collect::<Vec<_>>()
                .join("\n");
            bail!(
                "{} people on the team are called \"{given}\". Use the id of the one you mean:\n{ids}",
                several.len()
            )
        }
    }
}

fn same_name(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

async fn invite(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    name: String,
    spaces: Vec<TeamSpaceAccess>,
    url: Option<String>,
) -> Result<()> {
    let before = client.team_status().await?;
    // A second member with the same name could not be told apart by name
    // afterwards; a new link for the same person is `new-key`.
    if before
        .members
        .iter()
        .any(|member| member.removed_at.is_none() && same_name(&member.name, &name))
    {
        bail!(
            "{name} is already on the team. A new invite link for them: wenlan team new-key \"{name}\""
        );
    }
    let issued = client
        .team_add_member(&TeamAddMemberRequest { name, spaces, url })
        .await?;
    print_key(format, quiet, "Invited", &issued, Some(&before.everyone))
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

/// The Spaces of one joined team here, from everything this Wenlan syncs.
fn team_shares<'a>(shares: &'a [SharedSyncStatus], url: &str) -> Vec<&'a SharedSyncStatus> {
    shares
        .iter()
        .filter(|share| share.host.as_deref() == Some(url))
        .collect()
}

/// The teams this computer has joined: the ones the daemon keeps, plus any
/// server a Space still syncs with (a daemon older than the list route).
fn joined_teams(
    memberships: Vec<TeamMembershipInfo>,
    shares: &[SharedSyncStatus],
) -> Vec<TeamMembershipInfo> {
    let mut joined = memberships;
    for share in shares {
        let Some(host) = &share.host else { continue };
        if !joined.iter().any(|team| &team.url == host) {
            joined.push(TeamMembershipInfo {
                url: host.clone(),
                team: share.team.clone(),
                owner: false,
            });
        }
    }
    joined
}

fn team_label(team: &TeamMembershipInfo) -> String {
    match team.team.as_ref().and_then(|info| info.name.as_deref()) {
        Some(name) => format!("{name} ({})", team.url),
        None => team.url.clone(),
    }
}

async fn joined(client: &WenlanClient) -> Result<(Vec<TeamMembershipInfo>, Vec<SharedSyncStatus>)> {
    let shares = client.shared_list().await?;
    // An older daemon has no list route: the Spaces still say which servers.
    let memberships = client.team_memberships().await.unwrap_or_default();
    Ok((joined_teams(memberships, &shares), shares))
}

/// Whether this computer runs a team: named, listening, or with anyone or
/// anything on it. A fresh Wenlan answers `GET /api/team` with none of it.
fn runs_a_team(status: &TeamStatus) -> bool {
    status.remote.is_none()
        && (status.listening.is_some()
            || !status.spaces.is_empty()
            || !status.members.is_empty()
            || status.team.as_ref().is_some_and(|team| team.name.is_some()))
}

async fn status(client: &WenlanClient, format: ResolvedFormat, quiet: bool) -> Result<()> {
    let status = client.team_status().await?;
    let (joined, shares) = joined(client).await?;
    if quiet {
        return Ok(());
    }
    if format == ResolvedFormat::Json {
        let mut value = serde_json::to_value(&status)?;
        if let Some(object) = value.as_object_mut() {
            object.insert("joined".to_string(), serde_json::to_value(&joined)?);
        }
        return crate::output::print_json(&value);
    }
    let runs = runs_a_team(&status);
    if !runs && status.remote.is_none() && joined.is_empty() {
        println!("This computer is not on a team.");
        println!("  Start one: wenlan team start <NAME>");
        println!("  Join one:  wenlan team join <INVITE LINK>");
        return Ok(());
    }
    if runs || status.remote.is_some() {
        print_own_team(&status);
    }
    for team in &joined {
        // The team this person owns and manages from here is shown above.
        if status.remote.as_deref() == Some(team.url.as_str()) {
            continue;
        }
        println!("Joined: {}", team_label(team));
        let spaces = team_shares(&shares, &team.url);
        if spaces.is_empty() {
            println!("  No Spaces are open to you yet. They sync here when the team opens one.");
        }
        for share in spaces {
            let access = if share.read_only { " (read)" } else { "" };
            println!("  {}{access}", share.target);
        }
    }
    Ok(())
}

fn print_own_team(status: &TeamStatus) {
    match status.team.as_ref().and_then(|team| team.name.as_deref()) {
        Some(name) => println!("Team: {name}"),
        None => println!("Team: not named yet (wenlan team rename <NAME>)"),
    }
    match (&status.remote, &status.listening) {
        (Some(server), _) => println!("Runs on: {server} (you manage it from this computer)"),
        (None, Some(addr)) => println!("Listening on: {addr}"),
        (None, None) => {
            println!("Not started: others cannot reach it (wenlan team start <NAME>)")
        }
    }
    if let Some(url) = &status.url {
        println!("Invite links use: {url}");
    }
    if status.spaces.is_empty() {
        println!("Hosted Spaces: none (wenlan team share <SPACE>)");
    } else {
        println!("Hosted Spaces: {}", status.spaces.join(", "));
    }
    if !status.everyone.is_empty() {
        println!("Open to everyone: {}", access_list(&status.everyone));
    }
    let active: Vec<&TeamMember> = status
        .members
        .iter()
        .filter(|member| member.removed_at.is_none())
        .collect();
    if active.is_empty() {
        println!("No one invited yet (wenlan team invite <NAME>)");
        return;
    }
    println!("{:<20} SPACES", "NAME");
    for member in active {
        println!("{:<20} {}", member.name, access_list(&member.spaces));
    }
}

/// Prints the invite link (or the key). It is shown only this once.
/// `everyone` is the team's Spaces every member has, for saying what the
/// new person can use.
fn print_key(
    format: ResolvedFormat,
    quiet: bool,
    verb: &str,
    issued: &TeamMemberKey,
    everyone: Option<&[TeamSpaceAccess]>,
) -> Result<()> {
    // The link is the point of the command, so --quiet still prints it as
    // the one thing on stdout, never a status line.
    if quiet {
        println!("{}", issued.join_code.as_deref().unwrap_or(&issued.key));
        return Ok(());
    }
    match format {
        ResolvedFormat::Json => crate::output::print_json(issued)?,
        ResolvedFormat::Table => {
            println!("{} {}.", verb, issued.member.name);
            match &issued.join_code {
                Some(code) => println!("Invite link: {code}"),
                None => {
                    println!("Key: {}", issued.key);
                    println!("For a link they can open, give the address their computer reaches this one at: --url <ADDRESS>");
                }
            }
            println!("This is shown only once. Send it to them privately.");
            if let Some(everyone) = everyone {
                let mut spaces = issued.member.spaces.clone();
                for space in everyone {
                    if !spaces.iter().any(|own| own.space == space.space) {
                        spaces.push(space.clone());
                    }
                }
                if spaces.is_empty() {
                    println!(
                        "No Spaces are open to them yet. Open one to everyone: wenlan team share <SPACE> --everyone. Or to them: wenlan team update \"{}\" --access <SPACE>",
                        issued.member.name
                    );
                } else {
                    println!("They can use: {}", access_list(&spaces));
                }
            }
        }
    }
    Ok(())
}

fn print_open_spaces(joined: &TeamJoinResponse) {
    println!("Server: {}", joined.url);
    println!("You are {}.", joined.me.name);
    if joined.me.spaces.is_empty() {
        println!("No Spaces are open to you yet.");
        return;
    }
    println!("Spaces you can use:");
    for s in &joined.me.spaces {
        let access = match s.access {
            TeamAccess::Read => "read only",
            TeamAccess::Write => "read and write",
        };
        println!("  {} ({access})", s.space);
    }
}

/// `--list`: what the link opens. Nothing is joined.
async fn join_list(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    code: &str,
) -> Result<()> {
    let joined = client.team_join(code).await?;
    output(format, quiet, &joined, || {
        print_open_spaces(&joined);
        println!("Join them: wenlan team join <INVITE LINK>");
    })
}

/// The name a team's Space gets on this computer. A Space here with the
/// same name may hold this person's own pages: the team's copy goes beside
/// it as `<name> (<team>)`, never into it. A Space that already syncs with
/// this team keeps its name, so joining twice makes nothing new.
fn local_space_name(
    server_space: &str,
    team: &str,
    server: &str,
    here: &[String],
    shares: &[SharedSyncStatus],
) -> String {
    let syncs_with_team = |name: &str| {
        shares
            .iter()
            .any(|share| share.host.as_deref() == Some(server) && same_name(&share.target, name))
    };
    let taken = |name: &str| here.iter().any(|space| same_name(space, name));
    if !taken(server_space) || syncs_with_team(server_space) {
        return server_space.to_string();
    }
    format!("{server_space} ({team})")
}

/// One Space the join synced.
#[derive(serde::Serialize)]
struct JoinedSpace {
    /// Its name on the team's server.
    space: String,
    /// Its name on this computer.
    local: String,
    access: TeamAccess,
    counts: wenlan_types::shared_space::SharedSyncCounts,
}

#[derive(serde::Serialize)]
struct JoinedTeam {
    url: String,
    me: wenlan_types::team::TeamMe,
    spaces: Vec<JoinedSpace>,
}

/// Joins the team the way the app's Join does: keeps the membership (so
/// Spaces opened later follow), then syncs every Space open to this person.
async fn join_all(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    code: &str,
) -> Result<()> {
    let found = client.team_join(code).await?;
    let me = client.team_remember_membership(code).await?;
    let here: Vec<String> = client
        .list_spaces()
        .await?
        .into_iter()
        .map(|space| space.name)
        .collect();
    let shares = client.shared_list().await?;
    let team = me
        .team
        .as_ref()
        .and_then(|team| team.name.clone())
        .unwrap_or_else(|| found.url.clone());
    let mut spaces = Vec::new();
    for entry in &me.spaces {
        let local = local_space_name(&entry.space, &team, &found.url, &here, &shares);
        let counts = client
            .shared_sync(&local, &host_request(code, &entry.space))
            .await?;
        spaces.push(JoinedSpace {
            space: entry.space.clone(),
            local,
            access: entry.access,
            counts,
        });
    }
    let joined = JoinedTeam {
        url: found.url,
        me,
        spaces,
    };
    output(format, quiet, &joined, || {
        println!("Joined {team} as {}.", joined.me.name);
        if joined.spaces.is_empty() {
            println!("No Spaces are open to you yet. They sync here when the team opens one.");
        }
        for space in &joined.spaces {
            let access = match space.access {
                TeamAccess::Read => " (read only)",
                TeamAccess::Write => "",
            };
            let pages = match space.counts.applied {
                1 => "1 page taken".to_string(),
                count => format!("{count} pages taken"),
            };
            if space.local == space.space {
                println!("  {}{access}: {pages}", space.space);
            } else {
                println!(
                    "  {}{access}: {pages}, here as \"{}\"",
                    space.space, space.local
                );
            }
        }
    })
}

fn host_request(code: &str, server_space: &str) -> SharedSyncRequest {
    SharedSyncRequest {
        folder: String::new(),
        member_name: None,
        host: Some(SharedHost {
            code: code.to_string(),
            key: None,
            space: Some(server_space.to_string()),
        }),
        member_id: None,
    }
}

async fn join_one(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    code: &str,
    server_space: String,
    local_space: Option<String>,
) -> Result<()> {
    let local = local_space.unwrap_or_else(|| server_space.clone());
    let counts = client
        .shared_sync(&local, &host_request(code, &server_space))
        .await?;
    output(format, quiet, &counts, || {
        println!(
            "Synced '{}' with '{}' on the team server: {} sent, {} taken, {} edited at the same time, {} skipped.",
            local, server_space, counts.exported, counts.applied, counts.conflicts, counts.skipped
        );
    })
}

/// The joined team that `given` names: its server's address or the team's
/// name. With nothing given, the only team this computer is on.
fn pick_team<'a>(
    joined: &'a [TeamMembershipInfo],
    given: Option<&str>,
) -> Result<&'a TeamMembershipInfo> {
    let listed = || {
        joined
            .iter()
            .map(|team| format!("  {}", team_label(team)))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let Some(given) = given.map(str::trim).filter(|given| !given.is_empty()) else {
        return match joined {
            [] => bail!("this computer has not joined a team"),
            [team] => Ok(team),
            _ => bail!(
                "this computer is on {} teams. Say which: wenlan team leave <TEAM>\n{}",
                joined.len(),
                listed()
            ),
        };
    };
    let matches: Vec<&TeamMembershipInfo> = joined
        .iter()
        .filter(|team| {
            team.url.trim_end_matches('/') == given.trim_end_matches('/')
                || team
                    .team
                    .as_ref()
                    .and_then(|info| info.name.as_deref())
                    .is_some_and(|name| same_name(name, given))
        })
        .collect();
    match matches.as_slice() {
        [team] => Ok(team),
        [] if joined.is_empty() => bail!("this computer has not joined a team"),
        [] => bail!(
            "this computer is not on a team called \"{given}\". It is on:\n{}",
            listed()
        ),
        _ => bail!(
            "more than one team here is called \"{given}\". Use its address:\n{}",
            listed()
        ),
    }
}

#[derive(serde::Serialize)]
struct LeftTeam {
    url: String,
    /// The Spaces that stopped syncing; their pages stay here.
    spaces: Vec<String>,
}

/// Leaves a team the way the app does: every Space it shares stops syncing
/// here, then the team is forgotten. Pages already here stay.
async fn leave(
    client: &WenlanClient,
    format: ResolvedFormat,
    quiet: bool,
    given: Option<String>,
) -> Result<()> {
    let (joined, shares) = joined(client).await?;
    let team = pick_team(&joined, given.as_deref())?;
    let label = team_label(team);
    let mut spaces = Vec::new();
    for share in team_shares(&shares, &team.url) {
        client.shared_leave(&share.target).await?;
        spaces.push(share.target.clone());
    }
    client.team_forget_membership(&team.url).await?;
    let left = LeftTeam {
        url: team.url.clone(),
        spaces,
    };
    output(format, quiet, &left, || {
        println!("Left {label}.");
        if !left.spaces.is_empty() {
            println!(
                "Stopped syncing: {}. The pages already on this computer stay.",
                left.spaces.join(", ")
            );
        }
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

    fn member(id: &str, name: &str, removed: bool) -> TeamMember {
        TeamMember {
            id: id.into(),
            name: name.into(),
            spaces: vec![],
            added_at: "2026-11-15T00:00:00Z".into(),
            removed_at: removed.then(|| "2026-11-15T01:00:00Z".into()),
            role: Default::default(),
            sign_in: None,
        }
    }

    #[test]
    fn a_member_is_found_by_id_or_by_a_name_only_one_person_has() {
        let members = [
            member("m1", "Amy", false),
            member("m2", "Bo", true),
            member("m3", "Bo", false),
        ];
        assert_eq!(resolve_member(&members, "m2").unwrap().id, "m2");
        assert_eq!(resolve_member(&members, " amy ").unwrap().id, "m1");
        // A removed member's name does not make the name ambiguous.
        assert_eq!(resolve_member(&members, "Bo").unwrap().id, "m3");
        assert!(resolve_member(&members, "Cy").is_err());
    }

    #[test]
    fn two_people_with_one_name_are_never_picked_between() {
        let members = [member("m1", "Amy", false), member("m2", "amy", false)];
        let error = resolve_member(&members, "Amy").unwrap_err().to_string();
        assert!(error.contains("m1") && error.contains("m2"), "{error}");
    }

    fn share(target: &str, host: Option<&str>) -> SharedSyncStatus {
        serde_json::from_value(serde_json::json!({
            "kind": "space", "target": target, "folder": "/f", "host": host,
            "member_id": "m", "member_name": "Amy", "members": [], "last": null,
        }))
        .unwrap()
    }

    #[test]
    fn a_teams_space_goes_beside_an_own_space_of_the_same_name() {
        let here = vec!["Product".to_string()];
        let server = "http://team.test:7879";
        // Nothing here by that name: the team's name for it.
        assert_eq!(
            local_space_name("Notes", "Acme", server, &here, &[]),
            "Notes"
        );
        // This person's own Space has the name: the team's copy goes beside it.
        assert_eq!(
            local_space_name("Product", "Acme", server, &here, &[]),
            "Product (Acme)"
        );
        // It already syncs with this team: joining again makes nothing new.
        let synced = [share("Product", Some(server))];
        assert_eq!(
            local_space_name("Product", "Acme", server, &here, &synced),
            "Product"
        );
        // Syncing with another team does not count.
        let other = [share("Product", Some("http://other.test:7879"))];
        assert_eq!(
            local_space_name("Product", "Acme", server, &here, &other),
            "Product (Acme)"
        );
    }

    fn joined_team(url: &str, name: Option<&str>) -> TeamMembershipInfo {
        TeamMembershipInfo {
            url: url.into(),
            team: name.map(|name| wenlan_types::team::TeamInfo {
                id: url.into(),
                name: Some(name.into()),
            }),
            owner: false,
        }
    }

    #[test]
    fn the_team_to_leave_is_the_only_one_or_the_one_named() {
        let one = [joined_team("http://a.test:7879", Some("Acme"))];
        assert_eq!(pick_team(&one, None).unwrap().url, "http://a.test:7879");
        let two = [
            joined_team("http://a.test:7879", Some("Acme")),
            joined_team("http://b.test:7879", Some("Beta")),
        ];
        assert!(pick_team(&two, None).is_err());
        assert_eq!(
            pick_team(&two, Some("beta")).unwrap().url,
            "http://b.test:7879"
        );
        assert_eq!(
            pick_team(&two, Some("http://a.test:7879/")).unwrap().url,
            "http://a.test:7879"
        );
        assert!(pick_team(&two, Some("Gamma")).is_err());
        assert!(pick_team(&[], None).is_err());
    }

    #[test]
    fn a_space_that_still_syncs_names_a_team_the_daemon_did_not_list() {
        let shares = [
            share("Product", Some("http://a.test:7879")),
            share("Own", None),
        ];
        let joined = joined_teams(vec![], &shares);
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].url, "http://a.test:7879");
        // Listed by the daemon already: not listed twice.
        let listed = vec![joined_team("http://a.test:7879", Some("Acme"))];
        assert_eq!(joined_teams(listed, &shares).len(), 1);
    }

    #[test]
    fn everyone_takes_write_read_or_nobody() {
        assert_eq!(parse_everyone("Read").unwrap(), TeamEveryone::Read);
        assert_eq!(parse_everyone("nobody").unwrap(), TeamEveryone::Nobody);
        assert!(parse_everyone("all").is_err());
    }
}
