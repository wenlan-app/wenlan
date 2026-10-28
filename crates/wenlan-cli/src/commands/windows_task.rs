// SPDX-License-Identifier: Apache-2.0
//! Task Scheduler XML keeps the executable separate from its arguments.

pub(super) fn task_name(root: Option<&std::path::Path>) -> String {
    let Some(root) = root else {
        return "WenlanServer".into();
    };
    use sha2::{Digest, Sha256};
    format!(
        "WenlanServer-{:x}",
        Sha256::digest(normalized_root(root).as_bytes())
    )
}

pub(super) fn same_root(left: &std::path::Path, right: &std::path::Path) -> bool {
    normalized_root(left) == normalized_root(right)
}

fn normalized_root(root: &std::path::Path) -> String {
    let root = std::fs::canonicalize(root)
        .or_else(|_| std::path::absolute(root))
        .unwrap_or_else(|_| root.to_path_buf());
    let normalized = root.to_string_lossy().replace('\\', "/");
    let normalized = normalized.strip_prefix("//?/").unwrap_or(&normalized);
    let normalized = normalized
        .strip_prefix("UNC/")
        .or_else(|| normalized.strip_prefix("unc/"))
        .map(|path| format!("//{path}"))
        .unwrap_or_else(|| normalized.to_owned());
    normalized
}

fn xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Quote one Windows argv member, including trailing backslashes before `"`.
fn quote(value: &str) -> String {
    let mut out = String::from("\"");
    let mut slashes = 0;
    for ch in value.chars() {
        if ch == '\\' {
            slashes += 1;
            continue;
        }
        if ch == '"' {
            out.push_str(&"\\".repeat(slashes * 2 + 1));
        } else {
            out.push_str(&"\\".repeat(slashes));
        }
        slashes = 0;
        out.push(ch);
    }
    out.push_str(&"\\".repeat(slashes * 2));
    out.push('"');
    out
}

#[cfg(windows)]
pub(super) fn parse_arguments(arguments: &str) -> std::io::Result<Vec<String>> {
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "shell32")]
    extern "system" {
        fn CommandLineToArgvW(command: *const u16, count: *mut i32) -> *mut *mut u16;
    }
    #[link(name = "kernel32")]
    extern "system" {
        fn LocalFree(memory: *mut std::ffi::c_void) -> *mut std::ffi::c_void;
    }
    // argv[0] follows different quoting rules. Supply a harmless executable
    // and parse only its arguments using the Windows parser itself.
    let input: Vec<u16> = format!("wenlan-server.exe {arguments}")
        .encode_utf16()
        .chain(Some(0))
        .collect();
    let mut count = 0;
    // SAFETY: input is NUL-terminated; the OS owns a count-element argv array
    // and NUL-terminated strings until the matching LocalFree below.
    unsafe {
        let argv = CommandLineToArgvW(input.as_ptr(), &mut count);
        if argv.is_null() {
            return Err(std::io::Error::last_os_error());
        }
        let mut result = Vec::new();
        for index in 1..count {
            let value = *argv.add(index as usize);
            let mut length = 0;
            while *value.add(length) != 0 {
                length += 1;
            }
            result.push(
                std::ffi::OsString::from_wide(std::slice::from_raw_parts(value, length))
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        LocalFree(argv.cast());
        Ok(result)
    }
}

pub(super) fn task_xml(
    program: &str,
    data_root: Option<&str>,
    bind: std::net::SocketAddr,
) -> String {
    let mut args = vec![
        "--port".to_string(),
        bind.port().to_string(),
        "--bind-addr".to_string(),
        bind.to_string(),
    ];
    if let Some(root) = data_root {
        args.extend(["--data-dir".into(), root.into()]);
    }
    let arguments = args
        .iter()
        .map(|arg| quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        r#"<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
<Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
<Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><ExecutionTimeLimit>PT0S</ExecutionTimeLimit></Settings>
<Actions><Exec><Command>{}</Command><Arguments>{}</Arguments></Exec></Actions>
</Task>"#,
        xml(program),
        xml(&arguments)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[cfg(windows)]
    fn scheduler_parses_the_production_action_without_registering_a_task() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("definition.xml");
        let program = r"C:\Program Files\中文 & Co\wenlan-server.exe";
        std::fs::write(
            &path,
            task_xml(
                program,
                Some(r"D:\Notes & Data"),
                "127.0.0.1:17917".parse().unwrap(),
            ),
        )
        .unwrap();
        let result = std::process::Command::new("powershell.exe")
            .args(["-NoProfile", "-NonInteractive", "-Command", "$ErrorActionPreference='Stop'; $service=New-Object -ComObject Schedule.Service; $task=$service.NewTask(0); $task.XmlText=[IO.File]::ReadAllText($env:WENLAN_TASK_XML_FIXTURE); if ($task.Actions.Item(1).Path -cne $env:WENLAN_TASK_PROGRAM_FIXTURE) {throw 'executable path changed'}; if (-not $task.Actions.Item(1).Arguments.Contains('17917')) {throw 'bind address missing'}; Write-Output 'TASK_XML_OK'"])
            .env("WENLAN_TASK_XML_FIXTURE", &path).env("WENLAN_TASK_PROGRAM_FIXTURE", program)
            .output().unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(String::from_utf8_lossy(&result.stdout).contains("TASK_XML_OK"));
    }
    #[test]
    fn executable_and_runtime_are_independent_xml_fields() {
        let value = task_xml(
            r"C:\Program Files\中文 & Co\wenlan-server.exe",
            Some("D:\\Notes & Data\\"),
            "127.0.0.1:17917".parse().unwrap(),
        );
        assert!(
            value.contains(r"<Command>C:\Program Files\中文 &amp; Co\wenlan-server.exe</Command>")
        );
        assert!(value.contains("&quot;--bind-addr&quot; &quot;127.0.0.1:17917&quot;"));
        assert!(value.contains("&quot;D:\\Notes &amp; Data\\\\&quot;"));
    }
    #[test]
    fn argument_quotes_round_trip_trailing_slashes() {
        assert_eq!(quote("C:\\data\\"), "\"C:\\data\\\\\"");
        assert_eq!(quote("a\"b"), "\"a\\\"b\"");
    }
    #[test]
    #[cfg(windows)]
    fn arguments_round_trip_through_the_native_windows_parser() {
        let expected = vec![
            "--data-dir",
            "C:\\中文 Notes & Co\\",
            "a\"b",
            "--bind-addr",
            "127.0.0.1:17917",
        ];
        let encoded = expected
            .iter()
            .map(|arg| quote(arg))
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse_arguments(&encoded).unwrap(), expected);
    }
    #[test]
    fn custom_roots_do_not_control_the_default_task() {
        assert_eq!(task_name(None), "WenlanServer");
        let first = task_name(Some(std::path::Path::new("scratch-a")));
        assert_ne!(first, "WenlanServer");
        assert_ne!(first, task_name(Some(std::path::Path::new("scratch-b"))));
    }
    #[test]
    fn creating_the_data_root_cannot_change_the_task_name() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("new-root");
        let before = task_name(Some(&root));
        std::fs::create_dir(&root).unwrap();
        assert_eq!(before, task_name(Some(&root)));
    }
    #[test]
    fn case_sensitive_roots_are_not_collapsed_into_one_task() {
        let dir = tempfile::tempdir().unwrap();
        assert_ne!(
            task_name(Some(&dir.path().join("Case-A"))),
            task_name(Some(&dir.path().join("case-a")))
        );
    }
}
