// SPDX-License-Identifier: AGPL-3.0-only
//! Letting teammates through Windows Firewall to the team this computer
//! runs. On a network set to Public, Windows blocks a program the first time
//! it listens and asks nobody, so a team started here would report "on"
//! while no teammate can connect. Changing the firewall needs an
//! administrator, so this asks Windows once (its own prompt) and, with the
//! person's yes, removes Windows' blocks for this Wenlan's server and adds
//! one rule that lets it in. The daemon says whether it is needed
//! (`GET /api/team/firewall`).

use std::path::Path;

/// The rule this adds, by name, so a second allow replaces it.
#[cfg_attr(not(windows), allow(dead_code))]
const RULE_NAME: &str = "Wenlan team";

/// The program must be a Wenlan server at an absolute path, with nothing in
/// it PowerShell could read as the end of a quoted string.
fn checked_program(program: &str) -> Result<&str, String> {
    let path = Path::new(program);
    let named = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("wenlan-server.exe"));
    // PowerShell also takes the typographic single quotes as quotes.
    let plain = !program
        .chars()
        .any(|c| c.is_control() || matches!(c, '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}'));
    if !(path.is_absolute() && named && plain) {
        return Err("this is not Wenlan's server".to_string());
    }
    Ok(program)
}

/// The script run as administrator: Windows' own inbound blocks for the
/// program go (a block wins over any allow), and one rule lets it in on
/// work and private networks, and on public ones when `public_network`.
#[cfg_attr(not(windows), allow(dead_code))]
fn allow_script(program: &str, public_network: bool) -> String {
    let quoted = program.replace('\'', "''");
    let profiles = if public_network {
        "Domain,Private,Public"
    } else {
        "Domain,Private"
    };
    format!(
        "$ErrorActionPreference = 'Stop'\n\
         $exe = '{quoted}'\n\
         Get-NetFirewallApplicationFilter -Program $exe -ErrorAction SilentlyContinue | Get-NetFirewallRule | Where-Object {{ [string]$_.Direction -eq 'Inbound' -and ([string]$_.Action -eq 'Block' -or $_.DisplayName -eq '{RULE_NAME}') }} | Remove-NetFirewallRule\n\
         New-NetFirewallRule -DisplayName '{RULE_NAME}' -Description 'Lets teammates reach the team this Wenlan runs.' -Direction Inbound -Action Allow -Protocol TCP -Program $exe -Profile {profiles} | Out-Null\n"
    )
}

/// What `powershell -EncodedCommand` takes: the script as UTF-16LE, base64.
#[cfg_attr(not(windows), allow(dead_code))]
fn encoded(script: &str) -> String {
    use base64::Engine;
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Asks Windows (its administrator prompt) to let teammates in. `Err` when
/// the person said no, or the rules could not be changed.
#[cfg(windows)]
pub async fn allow(program: &str, public_network: bool) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let program = checked_program(program)?;
    if !Path::new(program).is_file() {
        return Err("this is not Wenlan's server".to_string());
    }
    let inner = encoded(&allow_script(program, public_network));
    // 1223 is Windows' "the operation was canceled by the user".
    let outer = format!(
        "try {{ $p = Start-Process -FilePath powershell.exe -Verb RunAs -WindowStyle Hidden -Wait -PassThru \
         -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{inner}'; \
         exit $p.ExitCode }} catch {{ exit 1223 }}"
    );
    let status = tokio::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
        ])
        .arg(outer)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .status()
        .await
        .map_err(|_| "Windows PowerShell could not be started".to_string())?;
    match status.code() {
        Some(0) => Ok(()),
        Some(1223) => Err("declined".to_string()),
        _ => Err("Windows Firewall could not be changed".to_string()),
    }
}

#[cfg(not(windows))]
pub async fn allow(program: &str, _public_network: bool) -> Result<(), String> {
    checked_program(program)?;
    Err("only Windows has this firewall".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_wenlan_server_path_is_taken() {
        assert!(
            checked_program("C:\\Users\\a\\AppData\\Local\\Programs\\Wenlan\\wenlan-server.exe")
                .is_ok()
                || cfg!(not(windows))
        );
        assert!(checked_program("wenlan-server.exe").is_err());
        assert!(checked_program("C:\\Windows\\System32\\cmd.exe").is_err());
        assert!(checked_program("C:\\x\u{2019}; calc; \u{2019}\\wenlan-server.exe").is_err());
        assert!(checked_program("C:\\x\n\\wenlan-server.exe").is_err());
    }

    #[test]
    fn the_script_quotes_the_path_and_opens_public_networks_only_when_asked() {
        let script = allow_script("C:\\O'Brien\\wenlan-server.exe", false);
        assert!(script.contains("$exe = 'C:\\O''Brien\\wenlan-server.exe'"));
        assert!(script.contains("-Profile Domain,Private |"));
        assert!(allow_script("C:\\w\\wenlan-server.exe", true)
            .contains("-Profile Domain,Private,Public |"));
        // UTF-16LE: "A" is 41 00.
        assert_eq!(encoded("A"), "QQA=");
    }
}
