// SPDX-License-Identifier: AGPL-3.0-only
//! Whether other computers get through Windows Firewall to this computer's
//! team listener. On a network set to Public, Windows blocks a program the
//! first time it listens and asks nobody, so a team started here reports
//! "on" while no teammate can connect. The app reads this to say so and to
//! offer the one Windows prompt that lets them in.
//!
//! The rules are read with the firewall's own PowerShell module, which needs
//! no administrator to read. Changing them does, so that is the app's: it can
//! show the prompt.
// Off Windows only the tests read the probe's parts.
#![cfg_attr(not(windows), allow(dead_code))]

use wenlan_types::team::TeamFirewall;

/// What the probe script prints: the Profile of each enabled inbound rule
/// for this program, by action, and the category of each network this
/// computer is on (`Public`, `Private`, `DomainAuthenticated`).
#[derive(Debug, Default, serde::Deserialize)]
pub(crate) struct Probe {
    #[serde(default, deserialize_with = "one_or_many")]
    block: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    allow: Vec<String>,
    #[serde(default, deserialize_with = "one_or_many")]
    categories: Vec<String>,
}

/// PowerShell prints a list of one as that one value, and none as `null`.
fn one_or_many<'de, D: serde::Deserializer<'de>>(de: D) -> Result<Vec<String>, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Shape {
        Many(Vec<String>),
        One(String),
        Nothing(()),
    }
    Ok(match <Shape as serde::Deserialize>::deserialize(de)? {
        Shape::Many(many) => many,
        Shape::One(one) => vec![one],
        Shape::Nothing(()) => Vec::new(),
    })
}

/// Whether a rule's Profile (`Any`, `Public`, `Private, Public`, …) covers a
/// network of `category`.
fn covers(profile: &str, category: &str) -> bool {
    let category = match category {
        "DomainAuthenticated" => "Domain",
        other => other,
    };
    profile
        .split(',')
        .map(str::trim)
        .any(|part| part.eq_ignore_ascii_case("Any") || part.eq_ignore_ascii_case(category))
}

impl Probe {
    /// Open when, on every network this computer is on, a rule lets the
    /// program in and none blocks it (a block wins over an allow). With no
    /// network at all, an allow and no block.
    pub(crate) fn verdict(&self, program: String) -> TeamFirewall {
        let open = if self.categories.is_empty() {
            self.block.is_empty() && !self.allow.is_empty()
        } else {
            self.categories.iter().all(|category| {
                !self.block.iter().any(|profile| covers(profile, category))
                    && self.allow.iter().any(|profile| covers(profile, category))
            })
        };
        TeamFirewall {
            program,
            open,
            public_network: self.categories.iter().any(|category| category == "Public"),
        }
    }
}

/// The probe, run by Windows PowerShell with the program's path in
/// `WENLAN_FW_PROGRAM` (no quoting of the path into the script). It reads
/// the active store: every rule in force, group policy's too, and about
/// six times faster than the persistent one. `-Program` matches the path
/// in any case.
pub(crate) const PROBE_SCRIPT: &str = r#"
$ErrorActionPreference = 'SilentlyContinue'
$exe = $env:WENLAN_FW_PROGRAM
$rules = @(Get-NetFirewallApplicationFilter -PolicyStore ActiveStore -Program $exe | Get-NetFirewallRule -PolicyStore ActiveStore | Where-Object { [string]$_.Enabled -eq 'True' -and [string]$_.Direction -eq 'Inbound' })
[pscustomobject]@{
  block = @($rules | Where-Object { [string]$_.Action -eq 'Block' } | ForEach-Object { [string]$_.Profile })
  allow = @($rules | Where-Object { [string]$_.Action -eq 'Allow' } | ForEach-Object { [string]$_.Profile })
  categories = @(Get-NetConnectionProfile | ForEach-Object { [string]$_.NetworkCategory })
} | ConvertTo-Json -Compress
"#;

/// This computer's answer: `None` off Windows, or when the rules cannot be
/// read (then nothing is claimed either way).
#[cfg(windows)]
pub(crate) async fn check() -> Option<TeamFirewall> {
    use std::os::windows::process::CommandExt;
    let program = std::env::current_exe().ok()?;
    let program = program.to_string_lossy().into_owned();
    let mut command = tokio::process::Command::new("powershell.exe");
    command
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(PROBE_SCRIPT)
        .env("WENLAN_FW_PROGRAM", &program)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    let output = tokio::time::timeout(std::time::Duration::from_secs(30), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        tracing::warn!("[team] the firewall rules could not be read");
        return None;
    }
    let probe: Probe = serde_json::from_slice(output.stdout.trim_ascii()).ok()?;
    Some(probe.verdict(program))
}

#[cfg(not(windows))]
pub(crate) async fn check() -> Option<TeamFirewall> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(json: &str) -> TeamFirewall {
        serde_json::from_str::<Probe>(json)
            .unwrap()
            .verdict("C:\\Wenlan\\wenlan-server.exe".into())
    }

    #[test]
    fn a_public_network_with_windows_own_block_is_closed() {
        // What Windows left on Qi-Xuan's PC: its silent block for Public.
        let seen = probe(r#"{"block":["Public","Public"],"allow":[],"categories":["Public"]}"#);
        assert!(!seen.open);
        assert!(seen.public_network);
        // PowerShell's shapes for one value and for none.
        let one = probe(r#"{"block":"Public","allow":null,"categories":"Public"}"#);
        assert!(!one.open);
    }

    #[test]
    fn an_allow_opens_only_the_networks_it_covers_and_a_block_wins() {
        assert!(probe(r#"{"block":[],"allow":["Private, Public"],"categories":["Public"]}"#).open);
        assert!(
            probe(r#"{"block":[],"allow":["Any"],"categories":["DomainAuthenticated","Private"]}"#)
                .open
        );
        assert!(!probe(r#"{"block":[],"allow":["Private"],"categories":["Public"]}"#).open);
        assert!(!probe(r#"{"block":["Public"],"allow":["Any"],"categories":["Public"]}"#).open);
        assert!(probe(r#"{"block":["Public"],"allow":["Private"],"categories":["Private"]}"#).open);
        // No rule at all: Windows asks or blocks; not open.
        assert!(!probe(r#"{"block":[],"allow":[],"categories":["Private"]}"#).open);
        assert!(!probe(r#"{"block":[],"allow":[],"categories":[]}"#).open);
    }
}
