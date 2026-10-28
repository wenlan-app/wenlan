// SPDX-License-Identifier: AGPL-3.0-only
//! Real updater HTTP, signature and macOS extraction against disposable fixtures.
//! Tauri's MockRuntime supplies an AppHandle without starting the Wenlan daemon/UI.
use super::*;
use std::sync::atomic::AtomicUsize;
use std::time::Instant;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::task::{JoinHandle, JoinSet};

const ARCHIVE: &[u8] = include_bytes!("../tests/fixtures/updater/fixture.app.tar.gz");
const SIGNATURE: &str = include_str!("../tests/fixtures/updater/fixture.app.tar.gz.sig");
const PUBLIC_KEY: &str = include_str!("../tests/fixtures/updater/fixture.pub");
const CURRENT_VERSION: &str = "0.19.0";
const OLD: &[u8] = b"Wenlan updater fixture v1\n";
const NEW: &[u8] = b"Wenlan updater fixture v2\n";

fn fault_log_has_event(log: &str, event: &str, result: i32, errno: Option<i32>) -> bool {
    log.lines().any(|line| {
        let mut fields = line.split_whitespace();
        fields.next() == Some(event)
            && fields
                .clone()
                .any(|field| field == format!("result={result}"))
            && match errno {
                Some(expected) => fields.any(|field| field == format!("errno={expected}")),
                None => true,
            }
    })
}

struct Server {
    endpoint: String,
    published: Arc<AtomicBool>,
    version: Arc<Mutex<String>>,
    fail_manifest: Arc<AtomicBool>,
    v1_downloads: Arc<AtomicUsize>,
    latest_downloads: Arc<AtomicUsize>,
    stall_manifest: Arc<AtomicBool>,
    stall_download: Arc<AtomicBool>,
    task: JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(tampered: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", listener.local_addr().unwrap());
        let endpoint = format!("{origin}/latest.json");
        let published = Arc::new(AtomicBool::new(false));
        let version = Arc::new(Mutex::new("0.21.0".to_string()));
        let fail_manifest = Arc::new(AtomicBool::new(false));
        let v1_downloads = Arc::new(AtomicUsize::new(0));
        let latest_downloads = Arc::new(AtomicUsize::new(0));
        let stall_manifest = Arc::new(AtomicBool::new(false));
        let stall_download = Arc::new(AtomicBool::new(false));
        let flags = (
            published.clone(),
            version.clone(),
            fail_manifest.clone(),
            v1_downloads.clone(),
            latest_downloads.clone(),
            stall_manifest.clone(),
            stall_download.clone(),
        );
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let (published, version, fail_manifest, v1_downloads, latest_downloads, stall_manifest, stall_download) = flags.clone();
                        let origin = origin.clone();
                        clients.spawn(async move {
                            let mut request = Vec::new();
                            let mut byte = [0u8; 1];
                            while !request.ends_with(b"\r\n\r\n") {
                                if socket.read_exact(&mut byte).await.is_err() { return; }
                                request.push(byte[0]);
                                assert!(request.len() < 16_384);
                            }
                            let download = request.starts_with(b"GET /fixture-v");
                            let requested_offer = request.starts_with(b"GET /fixture-v0.21.0.tar.gz ");
                            if requested_offer {
                                v1_downloads.fetch_add(1, Ordering::SeqCst);
                            } else if download {
                                latest_downloads.fetch_add(1, Ordering::SeqCst);
                            }
                            if !download && stall_manifest.swap(false, Ordering::SeqCst) {
                                std::future::pending::<()>().await;
                            }
                            let (status, mut body) = if download {
                                ("200 OK", ARCHIVE.to_vec())
                            } else if fail_manifest.swap(false, Ordering::SeqCst) {
                                ("503 Service Unavailable", Vec::new())
                            } else if published.load(Ordering::SeqCst) {
                                let version = version.lock().unwrap().clone();
                                ("200 OK", serde_json::to_vec(&serde_json::json!({
                                    "version": version, "url": format!("{origin}/fixture-v{version}.tar.gz"),
                                    "signature": SIGNATURE.trim(), "notes": "Disposable updater fixture"
                                })).unwrap())
                            } else { ("204 No Content", Vec::new()) };
                            if download && tampered { body.push(0); }
                            let headers = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            if socket.write_all(headers.as_bytes()).await.is_err() { return; }
                            if download && stall_download.load(Ordering::SeqCst) {
                                std::future::pending::<()>().await;
                            }
                            let _ = socket.write_all(&body).await;
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Self {
            endpoint,
            published,
            version,
            fail_manifest,
            v1_downloads,
            latest_downloads,
            stall_manifest,
            stall_download,
            task,
        }
    }
}

struct Fixture {
    app: tauri::App<MockRuntime>,
    root: PathBuf,
    _tempdir: Option<tempfile::TempDir>,
    executable: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        Self::at(root.path().join("Fixture.app"), Some(root))
    }
    fn existing(app_root: PathBuf) -> Self {
        Self::at(app_root, None)
    }
    fn at(root: PathBuf, tempdir: Option<tempfile::TempDir>) -> Self {
        let executable = root.join("Contents/MacOS/fixture");
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        if !executable.exists() {
            std::fs::write(&executable, OLD).unwrap();
        }
        let mut context = mock_context(noop_assets());
        context.config_mut().identifier = "com.wenlan.updater-fixture".into();
        context.package_info_mut().version = CURRENT_VERSION.parse().unwrap();
        context.config_mut().plugins.0.insert(
            "updater".into(),
            serde_json::json!({
                "dangerousInsecureTransportProtocol": true, "pubkey": PUBLIC_KEY.trim()
            }),
        );
        let app = mock_builder()
            .plugin(tauri_plugin_updater::Builder::new().build())
            .build(context)
            .unwrap();
        Self {
            app,
            root,
            _tempdir: tempdir,
            executable,
        }
    }
    fn updater(&self, server: &Server) -> tauri_plugin_updater::Updater {
        // The plugin's install destination comes from this explicit executable.
        // Never use current_exe(): only our temp directory may be replaced.
        assert!(self
            .executable
            .canonicalize()
            .unwrap()
            .starts_with(self.root.canonicalize().unwrap()));
        configure_network(self.app.updater_builder())
            .no_proxy()
            .executable_path(&self.executable)
            .endpoints(vec![server.endpoint.parse().unwrap()])
            .unwrap()
            .build()
            .unwrap()
    }
    fn publish(&self, server: &Server, version: &str) {
        *server.version.lock().unwrap() = version.to_string();
        server.published.store(true, Ordering::SeqCst);
    }
    fn assert_bytes(&self, expected: &[u8]) {
        assert_eq!(std::fs::read(&self.executable).unwrap(), expected);
    }
}

#[tokio::test]
async fn detects_a_release_without_restarting_and_installs_signed_fixture() {
    let server = Server::start(false).await;
    let fixture = Fixture::new();
    let updater = fixture.updater(&server);
    assert!(updater.check().await.unwrap().is_none());
    fixture.publish(&server, "99.0.0");
    let mut update = updater.check().await.unwrap().unwrap();
    assert_eq!(update.version, "99.0.0");
    fixture.assert_bytes(OLD);
    configure_download(&mut update);
    let bytes = Arc::new(AtomicUsize::new(0));
    let progress = bytes.clone();
    update
        .download_and_install(
            move |n, _| {
                progress.fetch_add(n, Ordering::SeqCst);
            },
            || {},
        )
        .await
        .unwrap();
    assert_eq!(bytes.load(Ordering::SeqCst), ARCHIVE.len());
    fixture.assert_bytes(NEW);
}

#[tokio::test]
async fn rejects_tampered_download_without_replacing_the_fixture() {
    let server = Server::start(true).await;
    let fixture = Fixture::new();
    fixture.publish(&server, "99.0.0");
    let mut update = fixture.updater(&server).check().await.unwrap().unwrap();
    configure_download(&mut update);
    assert!(update.download_and_install(|_, _| {}, || {}).await.is_err());
    fixture.assert_bytes(OLD);
}

#[tokio::test]
async fn stalled_manifest_times_out_and_a_later_check_succeeds() {
    let server = Server::start(false).await;
    server.stall_manifest.store(true, Ordering::SeqCst);
    let fixture = Fixture::new();
    let updater = fixture.updater(&server);
    let result = tokio::time::timeout(CHECK_TIMEOUT + Duration::from_secs(10), updater.check())
        .await
        .expect("manifest request outlived the production timeout");
    assert!(result.is_err());
    assert!(updater.check().await.unwrap().is_none());
    fixture.assert_bytes(OLD);
}

#[tokio::test]
async fn stalled_download_times_out_without_installing_and_can_retry() {
    let server = Server::start(false).await;
    let fixture = Fixture::new();
    fixture.publish(&server, "99.0.0");
    server.stall_download.store(true, Ordering::SeqCst);
    let mut update = fixture.updater(&server).check().await.unwrap().unwrap();
    configure_download(&mut update);
    let result = tokio::time::timeout(
        READ_TIMEOUT + Duration::from_secs(10),
        update.download_and_install(|_, _| {}, || {}),
    )
    .await
    .expect("download outlived the production read timeout");
    assert!(result.is_err());
    fixture.assert_bytes(OLD);
    server.stall_download.store(false, Ordering::SeqCst);
    update.download_and_install(|_, _| {}, || {}).await.unwrap();
    fixture.assert_bytes(NEW);
}

#[tokio::test]
async fn refreshed_manifest_skips_intermediate_releases_while_offer_is_open() {
    let server = Server::start(false).await;
    let fixture = Fixture::new();
    let updater = fixture.updater(&server);
    fixture.publish(&server, "0.21.0");

    // This is the manifest/version represented by an offer the user leaves
    // open while later stable releases are published.
    let stale_offer = updater.check().await.unwrap().unwrap();
    assert_eq!(stale_offer.version, "0.21.0");
    fixture.publish(&server, "0.24.0");

    let mut current = refresh_update(&updater).await.unwrap().unwrap();
    assert_eq!(current.version, "0.24.0");
    configure_download(&mut current);
    current
        .download_and_install(|_, _| {}, || {})
        .await
        .unwrap();

    assert_eq!(server.v1_downloads.load(Ordering::SeqCst), 0);
    assert_eq!(server.latest_downloads.load(Ordering::SeqCst), 1);
    fixture.assert_bytes(NEW);
    drop(stale_offer);
}

#[tokio::test]
async fn failed_refresh_never_falls_back_to_the_stale_asset_and_retries_latest() {
    let server = Server::start(false).await;
    let fixture = Fixture::new();
    let updater = fixture.updater(&server);
    fixture.publish(&server, "0.21.0");
    let stale_offer = updater.check().await.unwrap().unwrap();
    fixture.publish(&server, "0.24.0");
    server.fail_manifest.store(true, Ordering::SeqCst);

    assert!(refresh_update(&updater).await.is_err());
    assert_eq!(server.v1_downloads.load(Ordering::SeqCst), 0);
    assert_eq!(server.latest_downloads.load(Ordering::SeqCst), 0);
    fixture.assert_bytes(OLD);

    let mut retry = refresh_update(&updater).await.unwrap().unwrap();
    assert_eq!(retry.version, "0.24.0");
    configure_download(&mut retry);
    retry.download_and_install(|_, _| {}, || {}).await.unwrap();
    assert_eq!(server.v1_downloads.load(Ordering::SeqCst), 0);
    assert_eq!(server.latest_downloads.load(Ordering::SeqCst), 1);
    fixture.assert_bytes(NEW);
    drop(stale_offer);
}

#[tokio::test]
async fn an_exactly_current_manifest_is_treated_as_already_updated() {
    let server = Server::start(false).await;
    let fixture = Fixture::new();
    let updater = fixture.updater(&server);
    fixture.publish(&server, CURRENT_VERSION);

    assert!(updater.check().await.unwrap().is_none());
    assert_eq!(server.v1_downloads.load(Ordering::SeqCst), 0);
    assert_eq!(server.latest_downloads.load(Ordering::SeqCst), 0);
    fixture.assert_bytes(OLD);
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_install_failure_preserves_old_app_and_retry_succeeds() {
    const TARGET_ENV: &str = "WENLAN_UPDATER_FAULT_TARGET";
    const LOG_ENV: &str = "WENLAN_UPDATER_FAULT_LOG";
    const ARM_ENV: &str = "WENLAN_UPDATER_FAULT_ARM";
    const RESTORE_ARM_ENV: &str = "WENLAN_UPDATER_FAULT_RESTORE_ARM";
    const DYLIB_ENV: &str = "DYLD_INSERT_LIBRARIES";
    const CHILD_ENV: &str = "WENLAN_UPDATER_FAULT_CHILD";

    // The subprocess is selected by the exact test name and does the mutation.
    // This branch prevents recursively launching another child.
    if std::env::var(CHILD_ENV).as_deref() == Ok("1") {
        let target = std::env::var(TARGET_ENV).expect("child target path");
        let target = PathBuf::from(target);
        let fixture = Fixture::existing(target.clone());
        let server = Server::start(false).await;
        fixture.publish(&server, "99.0.0");
        let mut update = fixture.updater(&server).check().await.unwrap().unwrap();
        configure_download(&mut update);

        let failed = update.download_and_install(|_, _| {}, || {}).await;
        assert!(failed.is_err(), "injected replacement must fail");
        let log = std::fs::read_to_string(std::env::var(LOG_ENV).unwrap()).unwrap();
        eprintln!("macOS installer fault log:\n{log}");
        assert!(
            fault_log_has_event(&log, "rename_old_to_backup", 0, Some(0)),
            "fault log: {log}"
        );
        assert!(
            fault_log_has_event(&log, "rename_replacement_eio", -1, Some(5)),
            "fault log: {log}"
        );
        fixture.assert_bytes(OLD);
        assert!(!PathBuf::from(std::env::var(ARM_ENV).unwrap()).exists());
        assert!(
            fault_log_has_event(&log, "renamex_swap_enotsup", -1, None),
            "fault log: {log}"
        );
        assert!(
            fault_log_has_event(&log, "rename_restore_old_app", 0, Some(0)),
            "fault log: {log}"
        );

        // If the filesystem also refuses restoration, the only old bundle must
        // survive outside the self-deleting temporary directory, and the error
        // must identify it. Restore that scratch backup before retrying.
        std::fs::write(std::env::var(ARM_ENV).unwrap(), b"armed").unwrap();
        std::fs::write(std::env::var(RESTORE_ARM_ENV).unwrap(), b"armed").unwrap();
        let error = update
            .download_and_install(|_, _| {}, || {})
            .await
            .unwrap_err();
        let kept = target.with_file_name("Fixture (previous version).app");
        assert!(!target.exists());
        assert_eq!(
            std::fs::read(kept.join("Contents/MacOS/fixture")).unwrap(),
            OLD
        );
        assert!(error.to_string().contains(&kept.display().to_string()));
        let log = std::fs::read_to_string(std::env::var(LOG_ENV).unwrap()).unwrap();
        assert!(fault_log_has_event(&log, "rename_restore_eio", -1, Some(5)));
        std::fs::rename(kept, &target).unwrap();

        let mut retry = fixture.updater(&server).check().await.unwrap().unwrap();
        configure_download(&mut retry);
        retry.download_and_install(|_, _| {}, || {}).await.unwrap();
        fixture.assert_bytes(NEW);
        return;
    }

    let scratch = tempfile::tempdir().unwrap();
    let fixture_root = scratch.path().join("Fixture.app");
    let executable = fixture_root.join("Contents/MacOS/fixture");
    std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
    std::fs::write(&executable, OLD).unwrap();
    let interposer_source = include_str!("../tests/fixtures/updater/install-fault.c");
    let c_path = scratch.path().join("install-fault.c");
    let dylib_path = scratch.path().join("install-fault.dylib");
    let fault_log = scratch.path().join("fault.log");
    let arm_path = scratch.path().join("fault-armed");
    let restore_arm_path = scratch.path().join("restore-fault-armed");
    std::fs::write(&c_path, interposer_source).unwrap();
    let compile = std::process::Command::new("clang")
        .args(["-dynamiclib", "-o"])
        .arg(&dylib_path)
        .arg(&c_path)
        .output()
        .expect("clang is required for the macOS rollback fixture");
    assert!(
        compile.status.success(),
        "clang failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );
    std::fs::write(&arm_path, b"armed\n").unwrap();

    let module = module_path!()
        .split_once("::")
        .map(|(_, name)| name)
        .unwrap_or(module_path!());
    let test_name = format!("{module}::macos_install_failure_preserves_old_app_and_retry_succeeds");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &test_name, "--nocapture"])
        .env(TARGET_ENV, &fixture_root)
        .env(LOG_ENV, &fault_log)
        .env(ARM_ENV, &arm_path)
        .env(RESTORE_ARM_ENV, &restore_arm_path)
        .env(DYLIB_ENV, &dylib_path)
        .env(CHILD_ENV, "1")
        .env("WENLAN_NO_AUTOSTART", "1")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("launch bounded macOS rollback test child");
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let stdout_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut stdout, &mut bytes).unwrap();
        bytes
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut stderr, &mut bytes).unwrap();
        bytes
    });
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = stdout_reader.join().unwrap();
            let stderr = stderr_reader.join().unwrap();
            panic!(
                "macOS rollback test child exceeded 60 seconds:\n{}\n{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    };
    let stdout = stdout_reader.join().unwrap();
    let stderr = stderr_reader.join().unwrap();
    assert!(
        status.success(),
        "rollback child failed ({status}):\n{}\n{}",
        String::from_utf8_lossy(&stdout),
        String::from_utf8_lossy(&stderr)
    );
    let stdout = String::from_utf8_lossy(&stdout);
    assert!(
        stdout.contains("1 passed") && stdout.contains(&test_name),
        "child did not run exactly the rollback test:\n{stdout}\n{}",
        String::from_utf8_lossy(&stderr)
    );
}
