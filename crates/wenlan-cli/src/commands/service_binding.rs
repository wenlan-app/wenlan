// SPDX-License-Identifier: Apache-2.0
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct OwnershipRefusal(pub String);
impl std::fmt::Display for OwnershipRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Refusing lifecycle control: {}. Verify the existing service and its data directory, then follow docs/cross-platform.md (Legacy service upgrade); no automatic service fallback was attempted", self.0)
    }
}
impl std::error::Error for OwnershipRefusal {}

#[cfg(any(windows, test))]
pub fn verify_windows_stopped(state: u32, instances: u32) -> Result<(), OwnershipRefusal> {
    if instances == 0 && matches!(state, 1 | 3) {
        Ok(())
    } else {
        Err(OwnershipRefusal(
            "scheduled task is running, queued, or its runtime state is unknown".into(),
        ))
    }
}

#[cfg(any(target_os = "linux", test))]
pub fn verify_systemd_stopped(properties: &str) -> Result<(), OwnershipRefusal> {
    let value = |key: &str| -> Option<&str> {
        let mut values = properties.lines().filter_map(|line| line.strip_prefix(key));
        let value = values.next()?;
        if values.next().is_some() {
            return None;
        }
        Some(value.trim())
    };
    if matches!(value("ActiveState="), Some("inactive" | "failed"))
        && value("MainPID=") == Some("0")
    {
        Ok(())
    } else {
        Err(OwnershipRefusal(
            "systemd service is active or its runtime state is unknown".into(),
        ))
    }
}

/// launchctl's loaded configuration is independent of the plist on disk.
/// Only an explicit inactive state, no PID, and a matching effective launch
/// root can authorize starting that loaded job without replacing it.
#[cfg(any(target_os = "macos", test))]
pub fn verify_launchd_stopped(output: &str, selected: &Path) -> Result<(), OwnershipRefusal> {
    let refuse = || {
        OwnershipRefusal(
            "loaded launchd job is running or its effective ownership is unknown".into(),
        )
    };
    let mut state = None;
    let mut section = "";
    let mut args = Vec::new();
    let mut environment = Vec::new();
    for line in output.lines().map(str::trim) {
        if line.starts_with("pid =") {
            return Err(refuse());
        }
        if let Some(value) = line.strip_prefix("state = ") {
            if state.replace(value).is_some() {
                return Err(refuse());
            }
        }
        if line == "arguments = {" {
            section = "args";
            continue;
        }
        if line == "environment = {" {
            section = "env";
            continue;
        }
        if line == "}" {
            section = "";
            continue;
        }
        match section {
            "args" => args.push(line.to_owned()),
            "env" => {
                if let Some((key, value)) = line.split_once(" => ") {
                    environment.push((key.to_owned(), value.to_owned()));
                }
            }
            _ => {}
        }
    }
    if !matches!(state, Some("not running" | "exited")) || args.is_empty() {
        return Err(refuse());
    }
    verify_launch_root(selected, &args, &environment)
}

/// Command-line data-dir takes precedence over both environment spellings.
/// A missing marker is unproven, including older default-root registrations.
#[cfg_attr(windows, allow(dead_code))]
pub fn verify_launch_root(
    selected: &Path,
    args: &[String],
    environment: &[(String, String)],
) -> Result<(), OwnershipRefusal> {
    let refuse = |why: &str| OwnershipRefusal(why.into());
    for key in ["WENLAN_DATA_DIR", "ORIGIN_DATA_DIR"] {
        if environment.iter().filter(|(name, _)| name == key).count() > 1 {
            return Err(refuse("registered data-root environment repeats a key"));
        }
    }
    let mut explicit = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let value = if arg == "--data-dir" {
            Some(
                args.next()
                    .ok_or_else(|| refuse("registered --data-dir has no value"))?
                    .clone(),
            )
        } else {
            arg.strip_prefix("--data-dir=").map(str::to_owned)
        };
        if let Some(value) = value {
            if explicit.replace(value).is_some() {
                return Err(refuse("registered command repeats --data-dir"));
            }
        }
    }
    let root = explicit
        .or_else(|| {
            ["WENLAN_DATA_DIR", "ORIGIN_DATA_DIR"]
                .into_iter()
                .find_map(|key| {
                    environment
                        .iter()
                        .find(|(name, _)| name == key)
                        .map(|(_, value)| value.clone())
                })
        })
        .ok_or_else(|| refuse("legacy registration does not prove its data root"))?;
    let root = PathBuf::from(root);
    if !root.is_absolute() {
        return Err(refuse("registered data root is relative"));
    }
    let actual = std::fs::canonicalize(root)
        .map_err(|_| refuse("registered data root cannot be resolved"))?;
    let selected = std::fs::canonicalize(selected)
        .map_err(|_| refuse("selected data root cannot be resolved"))?;
    if actual != selected {
        return Err(refuse("service registration belongs to another data root"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updated_registration_cannot_authorize_stopping_a_running_old_instance() {
        assert!(verify_windows_stopped(4, 1).is_err());
        assert!(verify_windows_stopped(3, 1).is_err());
        assert!(verify_windows_stopped(2, 0).is_err());
        assert!(verify_windows_stopped(0, 0).is_err());
        assert!(verify_windows_stopped(3, 0).is_ok());
        assert!(verify_systemd_stopped("ActiveState=active\nMainPID=234\n").is_err());
        assert!(verify_systemd_stopped("ActiveState=inactive\nMainPID=234\n").is_err());
        assert!(verify_systemd_stopped("ActiveState=activating\nMainPID=0\n").is_err());
        assert!(verify_systemd_stopped("ActiveState=inactive\n").is_err());
        assert!(verify_systemd_stopped("ActiveState=inactive\nMainPID=0\n").is_ok());
    }
    #[test]
    fn loaded_launchd_root_is_checked_independently_of_the_plist() {
        let selected = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        let fixture = |root: &Path| {
            format!("service = {{\nstate = not running\narguments = {{\n/usr/bin/wenlan-server\n}}\nenvironment = {{\nWENLAN_DATA_DIR => {}\n}}\n}}\n", root.display())
        };
        assert!(verify_launchd_stopped(&fixture(selected.path()), selected.path()).is_ok());
        assert!(verify_launchd_stopped(&fixture(old.path()), selected.path()).is_err());
        assert!(verify_launchd_stopped(
            &(fixture(selected.path()) + "pid = 123\n"),
            selected.path()
        )
        .is_err());
        assert!(verify_launchd_stopped(
            &fixture(selected.path()).replace("not running", "running"),
            selected.path()
        )
        .is_err());
        assert!(verify_launchd_stopped("unreadable", selected.path()).is_err());
    }
    #[test]
    fn registration_requires_proof_and_honors_argument_precedence() {
        let one = tempfile::tempdir().unwrap();
        let two = tempfile::tempdir().unwrap();
        let env = vec![(
            "WENLAN_DATA_DIR".into(),
            one.path().to_string_lossy().into_owned(),
        )];
        assert!(verify_launch_root(one.path(), &[], &env).is_ok());
        assert!(verify_launch_root(two.path(), &[], &env).is_err());
        assert!(verify_launch_root(one.path(), &[], &[]).is_err());
        let args = vec![
            "--data-dir".into(),
            two.path().to_string_lossy().into_owned(),
        ];
        assert!(verify_launch_root(one.path(), &args, &env).is_err());
        assert!(verify_launch_root(two.path(), &args, &env).is_ok());
        assert!(verify_launch_root(one.path(), &["--data-dir".into()], &env).is_err());
    }
}
