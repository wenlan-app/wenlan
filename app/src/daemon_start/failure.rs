// SPDX-License-Identifier: AGPL-3.0-only
//! Keep the last launch failure after the child handle has been reaped.
#[derive(Default)]
pub(super) struct LaunchFailure {
    attempt: u64,
    message: Option<String>,
}

impl LaunchFailure {
    pub(super) const fn new() -> Self {
        Self {
            attempt: 0,
            message: None,
        }
    }

    pub(super) fn begin(&mut self) -> u64 {
        self.attempt += 1;
        self.message = None;
        self.attempt
    }

    pub(super) fn record(&mut self, attempt: u64, message: String) {
        if attempt == self.attempt {
            self.message = Some(message);
        }
    }

    pub(super) fn explain(&self, connection_error: String) -> String {
        match &self.message {
            Some(message) => format!("{message}\n{connection_error}"),
            None => connection_error,
        }
    }
}

pub(super) fn exit_message(code: Option<i32>, signal: Option<i32>, stderr: &str) -> String {
    let status = match code {
        Some(code) => format!("exit code {code} (0x{:08X})", code as u32),
        None => format!("signal {signal:?}"),
    };
    let hint = match code.map(|value| value as u32) {
        Some(0xC0000135) => " A required Windows runtime DLL is missing. Repair or reinstall Wenlan.",
        Some(0xC000007B) => " A Windows runtime DLL has the wrong architecture or is damaged. Repair or reinstall Wenlan.",
        Some(0xC0000142) => " Windows could not initialize a runtime DLL. Repair or reinstall Wenlan.",
        Some(0xC000001D) => " The processor cannot execute an instruction required by this build.",
        _ => "",
    };
    let detail: String = stderr.trim().chars().take(2048).collect();
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!("\n{detail}")
    };
    format!("wenlan-server stopped: {status}.{hint}{suffix}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failed_launch_survives_reaping_but_not_retry() {
        let mut state = LaunchFailure::new();
        let first = state.begin();
        state.record(first, exit_message(Some(0xC0000135_u32 as i32), None, ""));
        let error = state.explain("connection refused".into());
        assert!(error.contains("0xC0000135"));
        assert!(error.contains("DLL is missing"));
        assert!(error.contains("connection refused"));
        let second = state.begin();
        state.record(first, "late exit from old child".into());
        assert_eq!(state.explain("still starting".into()), "still starting");
        state.record(second, "access denied".into());
        assert!(state.explain("offline".into()).starts_with("access denied"));
    }

    #[test]
    fn generic_failures_keep_bounded_stderr_and_signal() {
        let message = exit_message(None, Some(9), &"字".repeat(4096));
        assert!(message.contains("signal Some(9)"));
        assert_eq!(message.matches('字').count(), 2048);
        assert!(!message.contains("DLL"));
    }
}
