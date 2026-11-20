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

/// The fields of a `launchctl print` job that ownership checks read.
#[cfg(any(target_os = "macos", test))]
struct LaunchdJob {
    state: Option<String>,
    pids: Vec<String>,
    args: Vec<String>,
    environment: Vec<(String, String)>,
}

#[cfg(any(target_os = "macos", test))]
fn parse_launchd_job(output: &str) -> Option<LaunchdJob> {
    let mut job = LaunchdJob {
        state: None,
        pids: Vec::new(),
        args: Vec::new(),
        environment: Vec::new(),
    };
    let mut section = "";
    // A running job nests coalition blocks with their own `state = active`;
    // only the job's own (outermost) keys describe the job.
    let mut depth = 0usize;
    for line in output.lines().map(str::trim) {
        let top_level = depth <= 1;
        // Dictionaries open with ` = {`, but nested values inside them (the
        // `LWCR` launch-constraint block of a signed job, for one) open with
        // ` => {`. Both close with `}`, so both must count, or the closing
        // brace of a `=> {` value drops the depth early and a later nested
        // `state = active` reads as a second job state.
        if line.ends_with(" = {") || line.ends_with(" => {") {
            depth += 1;
        } else if line == "}" {
            depth = depth.saturating_sub(1);
        }
        if let Some(value) = line.strip_prefix("pid = ").filter(|_| top_level) {
            job.pids.push(value.to_owned());
        }
        if let Some(value) = line.strip_prefix("state = ").filter(|_| top_level) {
            if job.state.replace(value.to_owned()).is_some() {
                return None;
            }
        }
        if top_level && line == "arguments = {" {
            section = "args";
            continue;
        }
        if top_level && line == "environment = {" {
            section = "env";
            continue;
        }
        if line == "}" {
            section = "";
            continue;
        }
        match section {
            "args" => job.args.push(line.to_owned()),
            "env" => {
                if let Some((key, value)) = line.split_once(" => ") {
                    job.environment.push((key.to_owned(), value.to_owned()));
                }
            }
            _ => {}
        }
    }
    Some(job)
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
    let job = parse_launchd_job(output).ok_or_else(refuse)?;
    if !job.pids.is_empty()
        || !matches!(job.state.as_deref(), Some("not running" | "exited"))
        || job.args.is_empty()
    {
        return Err(refuse());
    }
    verify_launch_root(selected, &job.args, &job.environment)
}

/// A daemon older than the data-root identity field can still be proven by
/// its supervisor: a running loaded job with exactly one PID whose effective
/// launch root is the selected one. Returns that PID.
#[cfg(any(target_os = "macos", test))]
pub fn verify_launchd_running_root(output: &str, selected: &Path) -> Result<u32, OwnershipRefusal> {
    let refuse = || {
        OwnershipRefusal(
            "legacy daemon does not report its data root and its launchd job could not prove it; stop the verified old instance manually before upgrading".into(),
        )
    };
    let job = parse_launchd_job(output).ok_or_else(refuse)?;
    let [pid] = job.pids.as_slice() else {
        return Err(refuse());
    };
    let pid: u32 = pid.parse().map_err(|_| refuse())?;
    if job.state.as_deref() != Some("running") || job.args.is_empty() {
        return Err(refuse());
    }
    verify_launch_root(selected, &job.args, &job.environment)?;
    Ok(pid)
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
    fn running_legacy_launchd_job_proves_its_root_and_pid() {
        let selected = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        let fixture = |root: &Path| {
            format!("service = {{\nstate = running\narguments = {{\n/Applications/Wenlan.app/Contents/MacOS/wenlan-server\n}}\nenvironment = {{\nWENLAN_DATA_DIR => {}\n}}\npid = 3451\n}}\n", root.display())
        };
        assert_eq!(
            verify_launchd_running_root(&fixture(selected.path()), selected.path()).unwrap(),
            3451
        );
        // Real `launchctl print` output nests coalitions with their own state.
        let coalitions = fixture(selected.path()).replace(
            "pid = 3451\n",
            "pid = 3451\nresource coalition = {\nstate = active\n}\njetsam coalition = {\nstate = active\n}\n",
        );
        assert_eq!(
            verify_launchd_running_root(&coalitions, selected.path()).unwrap(),
            3451
        );
        assert!(verify_launchd_running_root(&fixture(old.path()), selected.path()).is_err());
        assert!(verify_launchd_running_root(
            &fixture(selected.path()).replace("pid = 3451\n", ""),
            selected.path()
        )
        .is_err());
        assert!(verify_launchd_running_root(
            &(fixture(selected.path()) + "pid = 9999\n"),
            selected.path()
        )
        .is_err());
        assert!(verify_launchd_running_root(
            &fixture(selected.path()).replace("state = running", "state = exited"),
            selected.path()
        )
        .is_err());
        assert!(verify_launchd_running_root("unreadable", selected.path()).is_err());
    }
    /// The shape `launchctl print` gives a notarized release's job: the
    /// `LWCR` block nests ` => {` values, and the coalitions that follow
    /// carry their own `state = active`. Before the parser counted ` => {`,
    /// every stop/start of a signed install refused here.
    fn signed_job_fixture(root: &Path, state: &str, pid: Option<u32>) -> String {
        let pid = pid
            .map(|pid| format!("\tpid = {pid}\n"))
            .unwrap_or_default();
        format!(
            "gui/501/com.wenlan.server = {{\n\tactive count = 0\n\tpath = /Users/me/Library/LaunchAgents/com.wenlan.server.plist\n\ttype = LaunchAgent\n\tstate = {state}\n\n\tprogram = /Applications/Wenlan.app/Contents/MacOS/wenlan-server\n\targuments = {{\n\t\t/Applications/Wenlan.app/Contents/MacOS/wenlan-server\n\t}}\n\n\tinherited environment = {{\n\t\tSSH_AUTH_SOCK => /var/run/listeners\n\t}}\n\n\tdefault environment = {{\n\t\tPATH => /usr/bin:/bin:/usr/sbin:/sbin\n\t}}\n\n\tenvironment = {{\n\t\tRUST_LOG => info\n\t\tWENLAN_DATA_DIR => {root}\n\t\tXPC_SERVICE_NAME => com.wenlan.server\n\t}}\n\n\tLWCR = {{\n\t\t\"reqs\" => {{\n\t\t\t\"signing-identifier\" => \"wenlan-server\"\n\t\t\t\"team-identifier\" => \"TEAMID\"\n\t\t}}\n\t\t\"vers\" => 1\n\t}}\n\n\tdomain = gui/501 [100098]\n{pid}\truns = 1\n\tlast exit code = 0\n\n\tsemaphores = {{\n\t\tsuccessful exit => 0\n\t}}\n\n\tresource coalition = {{\n\t\tID = 4961\n\t\ttype = resource\n\t\tstate = active\n\t\tactive count = 1\n\t}}\n\n\tjetsam coalition = {{\n\t\tID = 4962\n\t\ttype = jetsam\n\t\tstate = active\n\t}}\n\n\tjob state = exited\n}}\n",
            root = root.display()
        )
    }
    #[test]
    fn signed_launchd_job_with_launch_constraints_is_parsed() {
        let selected = tempfile::tempdir().unwrap();
        let old = tempfile::tempdir().unwrap();
        let stopped = signed_job_fixture(selected.path(), "not running", None);
        assert!(verify_launchd_stopped(&stopped, selected.path()).is_ok());
        assert!(verify_launchd_stopped(
            &signed_job_fixture(old.path(), "not running", None),
            selected.path()
        )
        .is_err());
        let running = signed_job_fixture(selected.path(), "running", Some(7198));
        assert!(verify_launchd_stopped(&running, selected.path()).is_err());
        assert_eq!(
            verify_launchd_running_root(&running, selected.path()).unwrap(),
            7198
        );
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
