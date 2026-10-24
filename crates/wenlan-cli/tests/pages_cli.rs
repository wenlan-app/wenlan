// SPDX-License-Identifier: Apache-2.0
use assert_cmd::Command;
use serde_json::json;

mod common;

fn cli() -> Command {
    let mut command = Command::cargo_bin("wenlan").expect("wenlan binary built");
    command.env("WENLAN_NO_AUTOSTART", "1");
    command
}

/// An isolated data dir whose page folder holds the given `(file, markdown)` pages.
fn pages_root(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    let pages = root.path().join("pages");
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&pages).unwrap();
    std::fs::write(
        data.join("config.json"),
        serde_json::to_vec(&json!({ "knowledge_path": pages })).unwrap(),
    )
    .unwrap();
    for (name, body) in files {
        std::fs::write(pages.join(name), body).unwrap();
    }
    (root, data)
}

/// The stub route a daemon answers `GET /api/knowledge/path` with: the
/// `pages` folder next to `data`.
fn knowledge_path_route(data: &std::path::Path) -> (&'static str, String) {
    let pages = data.parent().unwrap().join("pages");
    (
        "GET /api/knowledge/path",
        json!({ "path": pages.to_string_lossy() }).to_string(),
    )
}

fn atlas_page_json() -> String {
    json!({ "page": {
        "id": "page_atlas",
        "title": "Project Atlas",
        "summary": null,
        "content": "Atlas ships in March.",
        "entity_id": null,
        "source_memory_ids": ["mem_0123456789"],
        "version": 1,
        "status": "active",
        "created_at": "2026-01-01T00:00:00Z",
        "last_compiled": "2026-01-01T00:00:00Z",
        "last_modified": "2026-01-01T00:00:00Z",
        "sources_updated_count": 0,
        "stale_reason": null,
        "user_edited": false,
        "storage_path": "project-atlas.md"
    }})
    .to_string()
}

fn atlas_sources_json() -> String {
    json!([{
        "source": {
            "page_id": "page_atlas",
            "memory_source_id": "mem_0123456789",
            "linked_at": 1
        },
        "memory": {
            "source_id": "mem_0123456789",
            "title": "Atlas date",
            "content": "The Atlas launch moved to March.",
            "summary": null,
            "memory_type": null,
            "source_agent": null,
            "confidence": null,
            "confirmed": true,
            "pinned": false,
            "supersedes": null,
            "last_modified": 1,
            "chunk_count": 1,
            "enrichment_status": "enriched",
    "supersede_mode": "hide",
    "access_count": 0
        }
    }])
    .to_string()
}

const ATLAS_MD: &str = "---\ntitle: Project Atlas\norigin_id: page_atlas\n---\n\nBody\n";

#[test]
fn pages_resolve_id_returns_frontmatter_origin_id_without_opening_the_page() {
    let (_root, data) = pages_root(&[(
        "project-atlas.md",
        "---\ntitle: Project Atlas\norigin_id: page_atlas\n---\n\nPrivate body\n",
    )]);
    let (host, _requests) = common::spawn_stub(vec![knowledge_path_route(&data)]);

    cli()
        .env("WENLAN_DATA_DIR", &data)
        .env("WENLAN_HOST", &host)
        .args(["pages", "Project Atlas", "--resolve-id"])
        .assert()
        .success()
        .stdout("page_atlas\n")
        .stderr("");
}

/// The folder comes from the daemon `WENLAN_HOST` selects, not from the local
/// config: the local config here points at a different folder holding other pages.
#[test]
fn pages_list_reads_the_folder_of_the_daemon_wenlan_host_selects() {
    let (_local_root, local_data) = pages_root(&[(
        "local.md",
        "---\ntitle: Local Config Page\norigin_id: page_local\n---\n",
    )]);
    let daemon_pages = tempfile::tempdir().unwrap();
    for (name, title) in [("alpha.md", "Daemon Alpha"), ("beta.md", "Daemon Beta")] {
        std::fs::write(
            daemon_pages.path().join(name),
            format!("---\ntitle: {title}\norigin_id: page_{title}\n---\n"),
        )
        .unwrap();
    }
    let (host, requests) = common::spawn_stub(vec![(
        "GET /api/knowledge/path",
        json!({ "path": daemon_pages.path().to_string_lossy() }).to_string(),
    )]);

    let output = cli()
        .env("WENLAN_DATA_DIR", &local_data)
        .env("WENLAN_HOST", &host)
        .args(["pages", "--format", "table"])
        .output()
        .expect("run pages");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("2 pages, 2 topics:"), "stdout: {stdout}");
    assert!(stdout.contains("Daemon Alpha") && stdout.contains("Daemon Beta"));
    assert!(!stdout.contains("Local Config Page"), "stdout: {stdout}");
    // A non-default daemon is named on stderr, with the folder listed.
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(&daemon_pages.path().to_string_lossy().to_string()),
        "stderr: {stderr}"
    );
    let served: Vec<String> = requests.try_iter().map(|(line, _)| line).collect();
    assert_eq!(served, ["GET /api/knowledge/path"]);
}

#[test]
fn pages_errors_when_the_selected_daemon_is_unreachable() {
    // A page exists in the local config's folder; it must not be listed instead.
    let (_root, data) = pages_root(&[("project-atlas.md", ATLAS_MD)]);
    // Bind then drop a listener so the port is free and nothing answers.
    let free_port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    cli()
        .env("WENLAN_DATA_DIR", &data)
        .env("WENLAN_HOST", format!("http://127.0.0.1:{free_port}"))
        .args(["pages"])
        .assert()
        .failure()
        .stdout("")
        .stderr(predicates::str::contains("/api/knowledge/path"));
}

#[test]
fn pages_print_shows_title_body_and_sources() {
    let (_root, data) = pages_root(&[("project-atlas.md", ATLAS_MD)]);
    let (host, requests) = common::spawn_stub(vec![
        knowledge_path_route(&data),
        ("GET /api/pages/page_atlas", atlas_page_json()),
        ("GET /api/pages/page_atlas/sources", atlas_sources_json()),
    ]);

    let output = cli()
        .env("WENLAN_DATA_DIR", &data)
        .env("WENLAN_HOST", &host)
        .args(["pages", "Project Atlas", "--print", "--format", "table"])
        .output()
        .expect("run pages --print");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "Project Atlas\n\nAtlas ships in March.\n\nSources:\n  mem_0123  The Atlas launch moved to March.\n"
    );
    let served: Vec<String> = requests.try_iter().map(|(line, _)| line).collect();
    assert_eq!(served.len(), 3);
}

#[test]
fn pages_print_json_is_one_object_with_page_and_sources() {
    let (_root, data) = pages_root(&[("project-atlas.md", ATLAS_MD)]);
    let (host, _requests) = common::spawn_stub(vec![
        knowledge_path_route(&data),
        ("GET /api/pages/page_atlas", atlas_page_json()),
        ("GET /api/pages/page_atlas/sources", atlas_sources_json()),
    ]);

    let output = cli()
        .env("WENLAN_DATA_DIR", &data)
        .env("WENLAN_HOST", &host)
        .args(["pages", "Project Atlas", "--print", "--format", "json"])
        .output()
        .expect("run pages --print --format json");
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["page"]["title"], "Project Atlas");
    assert_eq!(value["page"]["content"], "Atlas ships in March.");
    assert_eq!(
        value["sources"][0]["source"]["memory_source_id"],
        "mem_0123456789"
    );
}

#[test]
fn pages_print_with_several_matches_lists_them_and_fails_without_calling_out() {
    let (_root, data) = pages_root(&[
        (
            "atlas-one.md",
            "---\ntitle: Atlas One\norigin_id: page_1\n---\n",
        ),
        (
            "atlas-two.md",
            "---\ntitle: Atlas Two\norigin_id: page_2\n---\n",
        ),
    ]);
    let (host, _requests) = common::spawn_stub(vec![knowledge_path_route(&data)]);
    cli()
        .env("WENLAN_DATA_DIR", &data)
        .env("WENLAN_HOST", &host)
        .args(["pages", "atlas", "--print"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("2 pages match"))
        .stderr(predicates::str::contains("Atlas One"))
        .stderr(predicates::str::contains("Atlas Two"));
}

#[test]
fn pages_print_with_no_match_fails() {
    let (_root, data) = pages_root(&[("project-atlas.md", ATLAS_MD)]);
    let (host, _requests) = common::spawn_stub(vec![knowledge_path_route(&data)]);
    cli()
        .env("WENLAN_DATA_DIR", &data)
        .env("WENLAN_HOST", &host)
        .args(["pages", "zzz", "--print"])
        .assert()
        .failure()
        .stderr(predicates::str::contains("no page matches: zzz"));
}

#[test]
fn pages_print_conflicts_with_resolve_id_and_needs_a_query() {
    cli()
        .args(["pages", "x", "--print", "--resolve-id"])
        .assert()
        .failure();
    cli().args(["pages", "--print"]).assert().failure();
}
