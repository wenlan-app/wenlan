// SPDX-License-Identifier: Apache-2.0
//! Process-level proof that the database is only an index: delete it while
//! the daemon is stopped, start again, and what was saved is back, built
//! from the files.

#[path = "common/daemon_binary.rs"]
mod daemon_binary;

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

const STARTUP_READY_TIMEOUT: Duration = Duration::from_secs(120);
const MIRROR_TIMEOUT: Duration = Duration::from_secs(180);

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn logs(root: &Path, run: usize) -> String {
    let read = |name: &str| {
        std::fs::read_to_string(root.join(format!("daemon.{run}.{name}.log")))
            .unwrap_or_else(|error| format!("<{name}: {error}>"))
    };
    format!("stdout:\n{}\nstderr:\n{}", read("stdout"), read("stderr"))
}

/// Starts the daemon on `root/data` with its pages in `root/pages`.
async fn start(root: &Path, run: usize) -> (ChildGuard, u16) {
    let data_dir = root.join("data");
    let port_file = root.join(format!("port.{run}"));
    let mut command = Command::new(daemon_binary::wenlan_server_binary());
    command
        .env("WENLAN_BIND_ADDR", "127.0.0.1:0")
        .env("WENLAN_DATA_DIR", &data_dir)
        .env("WENLAN_PORT_FILE", &port_file)
        .env("WENLAN_NO_AUTOSTART", "1")
        .env("WENLAN_FILES_MIRROR_REFRESH_SECS", "1")
        .env_remove("WENLAN_FILES_MIRROR")
        .env_remove("WENLAN_RERANKER_MODE")
        .env_remove("WENLAN_RERANKER_ENABLED")
        .stdout(Stdio::from(
            std::fs::File::create(root.join(format!("daemon.{run}.stdout.log"))).unwrap(),
        ))
        .stderr(Stdio::from(
            std::fs::File::create(root.join(format!("daemon.{run}.stderr.log"))).unwrap(),
        ));
    // The search model from the CI cache, never a download.
    if let Some(cache) = std::env::var_os("WENLAN_TEST_FASTEMBED_CACHE")
        .or_else(|| std::env::var_os("FASTEMBED_CACHE_DIR"))
    {
        command.env("WENLAN_TEST_FASTEMBED_CACHE", cache);
    }
    let mut child = ChildGuard(command.spawn().expect("spawn wenlan-server"));
    let port = tokio::time::timeout(STARTUP_READY_TIMEOUT, async {
        loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                panic!("daemon exited at start: {status}\n{}", logs(root, run));
            }
            if let Ok(contents) = std::fs::read_to_string(&port_file) {
                if let Ok(port) = contents.trim().parse::<u16>() {
                    break port;
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("daemon did not start\n{}", logs(root, run)));
    (child, port)
}

async fn stop(client: &reqwest::Client, child: &mut ChildGuard, port: u16) {
    let response = client
        .post(format!("http://127.0.0.1:{port}/api/shutdown"))
        .send()
        .await
        .expect("shutdown");
    assert!(response.status().is_success());
    tokio::time::timeout(Duration::from_secs(30), async {
        while child.0.try_wait().unwrap().is_none() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("daemon did not stop");
}

fn database_files(data_dir: &Path) -> Vec<PathBuf> {
    let dir = data_dir.join("memorydb");
    std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.starts_with("origin_memory.db"))
                })
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_database_is_built_again_from_the_files() {
    let root = tempfile::tempdir().unwrap();
    let data_dir = root.path().join("data");
    let pages_dir = root.path().join("pages");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::create_dir_all(&pages_dir).unwrap();
    std::fs::write(
        data_dir.join("config.json"),
        serde_json::to_vec(&serde_json::json!({ "knowledge_path": pages_dir })).unwrap(),
    )
    .unwrap();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .unwrap();

    let (mut daemon, port) = start(root.path(), 1).await;
    let base = format!("http://127.0.0.1:{port}");
    let stored: serde_json::Value = client
        .post(format!("{base}/api/memory/store"))
        .json(&serde_json::json!({
            "content": "The lighthouse keeper logs the tide every dawn.",
            "memory_type": "fact"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let memory_id = stored["source_id"].as_str().unwrap().to_string();
    let created: serde_json::Value = client
        .post(format!("{base}/api/pages"))
        .json(&serde_json::json!({
            "title": "Lighthouse log",
            "content": "The keeper writes down the tide at dawn.",
            "creation_kind": "authored"
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    let page_id = created["id"].as_str().unwrap().to_string();
    let saved_at = chrono::Utc::now().timestamp();

    // The file copy follows the database within minutes; wait for one that
    // started after the saves above.
    tokio::time::timeout(MIRROR_TIMEOUT, async {
        loop {
            let status: serde_json::Value = client
                .get(format!("{base}/api/status"))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let mirror = &status["files_mirror"];
            if mirror["written_at"]
                .as_i64()
                .is_some_and(|at| at > saved_at)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("the files were never written\n{}", logs(root.path(), 1)));
    stop(&client, &mut daemon, port).await;

    let removed = database_files(&data_dir);
    assert!(!removed.is_empty(), "no database file to delete");
    for file in &removed {
        std::fs::remove_file(file).unwrap();
    }

    let (mut daemon, port) = start(root.path(), 2).await;
    let base = format!("http://127.0.0.1:{port}");
    assert!(
        data_dir.join("memorydb/origin_memory.db").exists(),
        "the database was not built again\n{}",
        logs(root.path(), 2)
    );
    let page: serde_json::Value = client
        .get(format!("{base}/api/pages/{page_id}"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap_or_else(|error| panic!("{error}\n{}", logs(root.path(), 2)))
        .json()
        .await
        .unwrap();
    let page = page.get("page").unwrap_or(&page);
    assert_eq!(page["title"], "Lighthouse log", "{page}");
    assert_eq!(
        page["content"], "The keeper writes down the tide at dawn.",
        "{page}"
    );
    let memory: serde_json::Value = client
        .get(format!("{base}/api/memory/{memory_id}/detail"))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap_or_else(|error| panic!("{error}\n{}", logs(root.path(), 2)))
        .json()
        .await
        .unwrap();
    assert!(
        memory
            .to_string()
            .contains("The lighthouse keeper logs the tide every dawn."),
        "{memory}"
    );
    stop(&client, &mut daemon, port).await;
}
