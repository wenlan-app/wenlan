// SPDX-License-Identifier: Apache-2.0
use std::{io, path::Path, process::Command};

/// The launcher is injected so tests inspect the exact commands used in the
/// product without opening applications or executing a fixture's metacharacters.
pub(super) fn open_document(
    path: &Path,
    mut launch: impl FnMut(&mut Command) -> io::Result<()>,
) -> bool {
    open::commands(path)
        .into_iter()
        .any(|mut command| launch(&mut command).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_paths_are_data_in_the_actual_launch_commands() {
        for raw in [
            r"C:\Notes&report.md",
            r"C:\中文 Notes\%name%!^(x).md",
            r"\\server\share\file.md",
        ] {
            let path = Path::new(raw);
            let mut called = false;
            assert!(open_document(path, |command| {
                called = true;
                let via_env = command
                    .get_envs()
                    .any(|(key, value)| key == "OPEN_RS_TARGET" && value == Some(path.as_os_str()));
                let via_arg = command.get_args().any(|arg| arg == path.as_os_str());
                assert!(
                    via_env || via_arg,
                    "the original path must reach the OS as one data value"
                );
                assert_ne!(command.get_program(), "cmd");
                assert_ne!(command.get_program(), "cmd.exe");
                Ok(())
            }));
            assert!(called);
        }
    }
}
