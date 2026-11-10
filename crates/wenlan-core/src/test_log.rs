// SPDX-License-Identifier: Apache-2.0
//! Test-only capture of `log` records, so a test can assert that a write path
//! said what it did. The logger is installed once for the test binary; each
//! capture collects only the records logged on its own thread (a
//! `#[tokio::test]` runs on its test's thread), so parallel tests do not see
//! each other's lines.

use std::cell::RefCell;

thread_local! {
    static LINES: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

struct ThreadCapture;

impl log::Log for ThreadCapture {
    fn enabled(&self, _: &log::Metadata<'_>) -> bool {
        true
    }

    fn log(&self, record: &log::Record<'_>) {
        LINES.with(|lines| {
            if let Some(lines) = lines.borrow_mut().as_mut() {
                lines.push(format!("{} {}", record.level(), record.args()));
            }
        });
    }

    fn flush(&self) {}
}

static LOGGER: ThreadCapture = ThreadCapture;

/// Collects this thread's log lines (`"<LEVEL> <message>"`) until dropped.
pub(crate) struct LogCapture;

impl LogCapture {
    pub(crate) fn start() -> Self {
        static INSTALL: std::sync::Once = std::sync::Once::new();
        INSTALL.call_once(|| {
            if log::set_logger(&LOGGER).is_ok() {
                log::set_max_level(log::LevelFilter::Debug);
            }
        });
        LINES.with(|lines| *lines.borrow_mut() = Some(Vec::new()));
        Self
    }

    pub(crate) fn lines(&self) -> Vec<String> {
        LINES.with(|lines| lines.borrow().clone().unwrap_or_default())
    }

    /// The captured lines at `level` (`"INFO"`, `"WARN"`, ...) that contain
    /// every one of `parts`.
    pub(crate) fn matching(&self, level: &str, parts: &[&str]) -> Vec<String> {
        self.lines()
            .into_iter()
            .filter(|line| line.starts_with(level) && parts.iter().all(|part| line.contains(part)))
            .collect()
    }
}

impl Drop for LogCapture {
    fn drop(&mut self) {
        LINES.with(|lines| *lines.borrow_mut() = None);
    }
}
