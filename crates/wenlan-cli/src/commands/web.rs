// SPDX-License-Identifier: Apache-2.0
//! Open the personal daemon's embedded browser UI.

use crate::client::WenlanClient;
use anyhow::Context;
use std::time::Duration;
#[cfg(unix)]
use std::time::Instant;

pub fn browser_url(base_url: &str) -> String {
    format!("{}/", base_url.trim_end_matches('/'))
}

/// The address that opens the page with a one-time browser pass: the daemon
/// forgets the pass and gives that one page a browser session token, never
/// the person key.
pub fn browser_url_with_pass(base_url: &str, pass: &str) -> String {
    format!("{}?pass={pass}", browser_url(base_url))
}

/// How long `wenlan open` waits for a browser to read the launcher file before
/// it gives up and returns (the daemon keeps the pass for a minute).
const BROWSER_WAIT: Duration = Duration::from_secs(60);

pub async fn run(client: &WenlanClient) -> anyhow::Result<()> {
    let plain = browser_url(client.base_url());
    // Only the person's own terminal asks for a pass; anything else opens
    // the page as an unnamed caller, which can do all but the person's
    // own actions.
    let pass = if client.names_an_agent() {
        None
    } else {
        client.browser_pass_as_person().await.ok()
    };
    println!("Wenlan in your browser: {plain}");
    let version = std::fs::read_to_string("/proc/version").unwrap_or_default();
    if !can_open_a_browser(
        |name| std::env::var_os(name).is_some_and(|value| !value.is_empty()),
        &version,
    ) {
        println!("No desktop session was found here, so no browser was opened. Open the address above in a browser.");
        return Ok(());
    }
    match pass {
        Some(answer) => open_with_pass(&browser_url_with_pass(client.base_url(), &answer.pass))
            .with_context(|| format!("open {plain} in the default browser")),
        None => {
            println!(
                "This page cannot tell it is you, so installing skills and accepting cards stay \
                 in the Wenlan app."
            );
            open::that_detached(&plain)
                .with_context(|| format!("open {plain} in the default browser"))
        }
    }
}

/// Whether a browser can be started from here. On Linux that needs a display
/// or a `BROWSER` command, or WSL (where opening goes through Windows);
/// elsewhere the system always has one to try.
fn can_open_a_browser(has_var: impl Fn(&str) -> bool, version_text: &str) -> bool {
    !cfg!(target_os = "linux")
        || ["DISPLAY", "WAYLAND_DISPLAY", "BROWSER", "WSL_DISTRO_NAME"]
            .iter()
            .any(|name| has_var(name))
        || version_text.to_ascii_lowercase().contains("microsoft")
}

/// Unix: the address with the pass never goes on a command line, which
/// other accounts on this computer can read (`/proc/*/cmdline`). It goes in
/// a launcher file only this account can read, the browser opens the file,
/// and the file is removed once the browser read it (at the latest when the
/// pass runs out).
#[cfg(unix)]
fn open_with_pass(url: &str) -> anyhow::Result<()> {
    let launcher = Launcher::write(url)?;
    open::that_detached(launcher.path())?;
    if !launcher.remove_once_read(BROWSER_WAIT) {
        println!("No browser opened the page. Open the address above in a browser yourself.");
    }
    Ok(())
}

/// Windows: the address goes to the browser directly. Another account
/// without administrator rights cannot read this account's process command
/// lines there, so the pass stays with this account.
#[cfg(not(unix))]
fn open_with_pass(url: &str) -> anyhow::Result<()> {
    open::that_detached(url)?;
    Ok(())
}

/// A small page, in a folder only this account can open (`0700`) and a file
/// only it can read (`0600`), that moves the browser on to the address with
/// the pass. Removed with its folder when dropped.
#[cfg(unix)]
struct Launcher {
    folder: tempfile::TempDir,
    file: std::path::PathBuf,
}

#[cfg(unix)]
impl Launcher {
    fn write(url: &str) -> anyhow::Result<Self> {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let folder = tempfile::Builder::new()
            .prefix("wenlan-open-")
            .tempdir()
            .context("making a private folder for the browser launcher")?;
        std::fs::set_permissions(folder.path(), std::fs::Permissions::from_mode(0o700))?;
        let file = folder.path().join("open-wenlan.html");
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&file)?;
        // The address is the daemon's own (a loopback host and port) and a
        // hex pass: nothing in it needs escaping.
        write!(
            out,
            "<!doctype html><meta charset=\"utf-8\"><meta name=\"referrer\" content=\"no-referrer\">\
             <meta http-equiv=\"refresh\" content=\"0;url={url}\"><title>Wenlan</title>\
             <p>Opening Wenlan\u{2026}</p>"
        )?;
        out.sync_all()?;
        Ok(Self { folder, file })
    }

    fn path(&self) -> &std::path::Path {
        &self.file
    }

    /// Wait until the browser read the file (its access time moves past the
    /// time it was written), or `limit` passes, then remove it.
    fn remove_once_read(self, limit: Duration) -> bool {
        let started = Instant::now();
        let mut was_read = false;
        while started.elapsed() < limit {
            let read = std::fs::metadata(&self.file)
                .and_then(|meta| Ok(meta.accessed()? > meta.modified()?))
                .unwrap_or(true);
            if read {
                // A moment for the browser to finish with it.
                std::thread::sleep(Duration::from_millis(500));
                was_read = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        drop(self.folder);
        was_read
    }
}

#[cfg(test)]
mod tests {
    /// UX-SKILL-13: the launcher that carries the pass is readable only by
    /// this account, says nothing but where to go, and is gone afterwards.
    #[cfg(unix)]
    #[test]
    fn ux_skill_13_launcher_file_is_owner_only_and_removed() {
        use std::os::unix::fs::PermissionsExt;
        let url = super::browser_url_with_pass("http://127.0.0.1:7878", &"ab".repeat(32));
        let launcher = super::Launcher::write(&url).unwrap();
        let file = launcher.path().to_path_buf();
        let folder = file.parent().unwrap().to_path_buf();
        let mode =
            |path: &std::path::Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&file), 0o600);
        assert_eq!(mode(&folder), 0o700);
        let page = std::fs::read_to_string(&file).unwrap();
        assert!(page.contains(&format!("url={url}")), "{page}");
        let started = std::time::Instant::now();
        // This test read the file above, as a browser would.
        assert!(launcher.remove_once_read(std::time::Duration::from_secs(5)));
        assert!(started.elapsed() < std::time::Duration::from_secs(6));
        assert!(!file.exists());
        assert!(!folder.exists());
    }

    #[test]
    fn ux_skill_13_browser_pass_address_opens_the_root_page() {
        assert_eq!(
            super::browser_url_with_pass("http://127.0.0.1:7878/", "ab"),
            "http://127.0.0.1:7878/?pass=ab"
        );
    }

    /// No browser reads the launcher: `wenlan open` gives up at the limit,
    /// says so, and leaves no folder behind.
    #[cfg(unix)]
    #[test]
    fn a_launcher_nobody_reads_is_removed_when_the_wait_ends() {
        let url = super::browser_url_with_pass("http://127.0.0.1:7878", "ab");
        let launcher = super::Launcher::write(&url).unwrap();
        let folder = launcher.path().parent().unwrap().to_path_buf();
        let started = std::time::Instant::now();
        assert!(!launcher.remove_once_read(std::time::Duration::from_secs(1)));
        assert!(started.elapsed() < std::time::Duration::from_secs(3));
        assert!(!folder.exists());
    }

    #[test]
    fn ux_cli_9_a_headless_linux_session_opens_no_browser() {
        let none = |_: &str| false;
        let display = |name: &str| name == "DISPLAY";
        if cfg!(target_os = "linux") {
            assert!(!super::can_open_a_browser(none, "Linux version 6.1 (gcc)"));
            assert!(super::can_open_a_browser(display, ""));
            // WSL opens the page through Windows.
            assert!(super::can_open_a_browser(
                none,
                "Linux ... microsoft-standard-WSL2"
            ));
            assert!(super::can_open_a_browser(
                |name| name == "WSL_DISTRO_NAME",
                ""
            ));
        } else {
            assert!(super::can_open_a_browser(none, ""));
        }
    }

    #[test]
    fn browser_address_has_one_trailing_slash() {
        assert_eq!(
            super::browser_url("http://127.0.0.1:7878"),
            "http://127.0.0.1:7878/"
        );
        assert_eq!(
            super::browser_url("http://127.0.0.1:7878/"),
            "http://127.0.0.1:7878/"
        );
    }
}
