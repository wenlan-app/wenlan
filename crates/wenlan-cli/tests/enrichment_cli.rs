// SPDX-License-Identifier: Apache-2.0
//! Cross-surface consent contract for the Wenlan CLI.

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

fn cli(base: &str) -> Command {
    let mut cmd = Command::cargo_bin("wenlan").expect("wenlan binary built");
    cmd.env("WENLAN_NO_AUTOSTART", "1");
    cmd.env("WENLAN_HOST", base);
    cmd
}

fn response(body: &str) -> String {
    response_with_status("200 OK", body)
}

fn response_with_status(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
}

fn spawn_stub(responses: Vec<String>) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind consent stub");
    let base = format!("http://{}", listener.local_addr().expect("stub address"));
    let (sent, received) = mpsc::channel();
    thread::spawn(move || {
        for response in responses {
            let (stream, _) = listener.accept().expect("accept consent request");
            let mut reader = BufReader::new(stream);
            let mut request = String::new();
            reader.read_line(&mut request).expect("request line");
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).expect("request header");
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    content_length = value.trim().parse().expect("content length");
                }
            }
            let mut body = vec![0u8; content_length];
            reader.read_exact(&mut body).expect("request body");
            request.push_str(&String::from_utf8(body).expect("utf8 body"));
            sent.send(request).expect("record request");
            reader
                .get_mut()
                .write_all(response.as_bytes())
                .expect("write response");
        }
    });
    (base, received)
}

fn routing(mode_everyday: &str, mode_synthesis: &str) -> String {
    serde_json::json!({
        "everyday": {
            "source": "anthropic",
            "model": "claude-haiku-4-5-20251001",
            "mode": mode_everyday,
            "pin": if mode_everyday == "unconfigured" { serde_json::Value::Null } else { serde_json::json!("anthropic") }
        },
        "synthesis": {
            "source": "on_device",
            "model": if mode_synthesis == "pinned" { serde_json::json!("qwen3-4b") } else { serde_json::Value::Null },
            "mode": mode_synthesis,
            "pin": if mode_synthesis == "unconfigured" { serde_json::Value::Null } else { serde_json::json!("on_device") }
        },
        "pool": {
            "anthropic": { "configured": true, "everyday_model": "claude-haiku-4-5-20251001", "synthesis_model": "claude-sonnet-4-6" },
            "external": null,
            "on_device": { "selected": "qwen3-4b", "loaded": mode_synthesis == "pinned", "loading": false }
        }
    })
    .to_string()
}

#[test]
fn status_uses_canonical_ready_paused_off_vocabulary() {
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":true}"#),
        response(&routing("pinned", "pinned_unavailable")),
    ]);

    cli(&base)
        .args(["steep", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Everyday organization: ready [anthropic]",
        ))
        .stdout(predicate::str::contains(
            "Page synthesis: paused (exact source unavailable; no fallback) [on_device]",
        ));

    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
}

#[test]
fn configure_discloses_and_writes_the_exact_confirmed_mapping() {
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":false}"#),
        response(&routing("unconfigured", "unconfigured")),
        response("{}"),
        response(
            r#"{"background_ai_enabled":true,"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
        ),
        response(r#"{"background_ai_enabled":true}"#),
        response(&routing("pinned", "pinned")),
    ]);

    cli(&base)
        .args([
            "steep",
            "configure",
            "--everyday",
            "anthropic",
            "--synthesis",
            "on-device",
            "--yes",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("Everyday organization: Anthropic"))
        .stdout(predicate::str::contains("On-device work uses CPU/GPU/RAM"))
        .stdout(predicate::str::contains(
            "Anthropic receives relevant memory content",
        ));

    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
    let put = requests.recv().unwrap();
    assert!(put.starts_with("PUT /api/config "), "{put}");
    assert!(put.contains(r#""background_ai_enabled":true"#), "{put}");
    assert!(put.contains(r#""everyday_source":"anthropic""#), "{put}");
    assert!(put.contains(r#""synthesis_source":"on_device""#), "{put}");
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
}

#[test]
fn disable_preserves_both_pins_without_removing_providers() {
    let (base, requests) = spawn_stub(vec![
        response(
            r#"{"background_ai_enabled":true,"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
        ),
        response("{}"),
        response(
            r#"{"background_ai_enabled":false,"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
        ),
    ]);

    cli(&base)
        .args(["steep", "disable"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Providers and downloaded models were kept",
        ));

    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    let put = requests.recv().unwrap();
    assert!(put.starts_with("PUT /api/config "), "{put}");
    assert!(put.contains(r#""background_ai_enabled":false"#), "{put}");
    assert!(!put.contains("everyday_source"), "{put}");
    assert!(!put.contains("synthesis_source"), "{put}");
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
}

#[test]
fn disable_surfaces_live_daemon_rejection_without_overwriting_disk() {
    let data = tempfile::tempdir().expect("consent data dir");
    let config_path = data.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
    )
    .expect("seed pinned config");
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":true}"#),
        response_with_status("500 Internal Server Error", r#"{"error":"write rejected"}"#),
    ]);

    cli(&base)
        .env("WENLAN_DATA_DIR", data.path())
        .args(["steep", "disable"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("HTTP 500"));

    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests.recv().unwrap().starts_with("PUT /api/config "));
    let config = std::fs::read_to_string(config_path).expect("config remains readable");
    assert!(
        config.contains(r#""everyday_source":"anthropic""#),
        "{config}"
    );
    assert!(
        config.contains(r#""synthesis_source":"on_device""#),
        "{config}"
    );
}

#[test]
fn disable_clears_local_pins_and_explicitly_opts_out_when_daemon_is_unreachable() {
    let data = tempfile::tempdir().expect("consent data dir");
    let config_path = data.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
    )
    .expect("seed pinned config");
    let listener = TcpListener::bind("127.0.0.1:0").expect("reserve unreachable port");
    let base = format!("http://{}", listener.local_addr().expect("stub address"));
    drop(listener);

    cli(&base)
        .env("WENLAN_DATA_DIR", data.path())
        .args(["steep", "disable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Steep disabled in local config"));

    let config: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(config_path).expect("config remains readable"),
    )
    .expect("saved config json");
    assert!(config["everyday_source"].is_null(), "{config}");
    assert!(config["synthesis_source"].is_null(), "{config}");
    assert_eq!(config["background_ai_enabled"], false, "{config}");
}

#[test]
fn status_reports_on_demand_with_preserved_ready_pins() {
    let (base, requests) = spawn_stub(vec![response(
        r#"{"background_ai_enabled":false,"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
    )]);
    cli(&base)
        .args(["steep", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Steep: On demand"))
        .stdout(predicate::str::contains("ready").not());
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
}

#[test]
fn status_does_not_claim_on_demand_or_ready_when_consent_is_unknown() {
    for reply in [
        response("{}"),
        response_with_status("500 Internal Server Error", "{}"),
    ] {
        let (base, requests) = spawn_stub(vec![reply]);
        cli(&base)
            .args(["steep", "status"])
            .assert()
            .success()
            .stdout(predicate::str::contains("Steep: status unavailable"))
            .stdout(predicate::str::contains("Steep: On demand").not())
            .stdout(predicate::str::contains("ready").not());
        assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    }
}

#[test]
fn status_preserves_legacy_enabled_preference_exposed_as_null() {
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":null}"#),
        response(&routing("pinned", "unconfigured")),
    ]);
    cli(&base)
        .args(["steep", "status"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Everyday organization: ready [anthropic]",
        ))
        .stdout(predicate::str::contains("Page synthesis: off"));
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
}

#[test]
fn configure_rejects_older_daemon_before_writing_pins() {
    let (base, requests) = spawn_stub(vec![response("{}")]);
    cli(&base)
        .args([
            "steep",
            "configure",
            "--everyday",
            "anthropic",
            "--synthesis",
            "on-device",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "does not support explicit background consent",
        ))
        .stdout(predicate::str::contains("consent saved").not());
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
}

#[test]
fn configure_does_not_claim_success_when_consent_write_is_ignored() {
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":false}"#),
        response(&routing("unconfigured", "unconfigured")),
        response("{}"),
        response(
            r#"{"background_ai_enabled":false,"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
        ),
    ]);
    cli(&base)
        .args([
            "steep",
            "configure",
            "--everyday",
            "anthropic",
            "--synthesis",
            "on-device",
            "--yes",
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "did not verify the requested background consent",
        ))
        .stdout(predicate::str::contains("consent saved").not());
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
    assert!(requests.recv().unwrap().starts_with("PUT /api/config "));
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
}

#[test]
fn disable_legacy_daemon_clears_pins_and_verifies_inactive_routes() {
    let (base, requests) = spawn_stub(vec![
        response("{}"),
        response("{}"),
        response(&routing("unconfigured", "unconfigured")),
    ]);
    cli(&base)
        .args(["steep", "disable"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Steep disabled"));
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    let put = requests.recv().unwrap();
    assert!(put.contains(r#""everyday_source":"""#), "{put}");
    assert!(put.contains(r#""synthesis_source":"""#), "{put}");
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
}

#[test]
fn disable_does_not_claim_success_when_current_or_legacy_daemon_keeps_work_enabled() {
    for replies in [
        vec![
            response(r#"{"background_ai_enabled":true}"#),
            response("{}"),
            response(r#"{"background_ai_enabled":true}"#),
        ],
        vec![
            response("{}"),
            response("{}"),
            response(&routing("pinned", "pinned")),
        ],
    ] {
        let (base, _requests) = spawn_stub(replies);
        cli(&base)
            .args(["steep", "disable"])
            .assert()
            .failure()
            .stdout(predicate::str::contains("Steep disabled").not());
    }
}

#[test]
fn basic_setup_explicitly_disables_previously_enabled_background_work() {
    let data = tempfile::tempdir().expect("basic setup data dir");
    let config_path = data.path().join("config.json");
    std::fs::write(
        &config_path,
        r#"{"background_ai_enabled":true,"everyday_source":"anthropic","synthesis_source":"on_device"}"#,
    )
    .expect("seed opted-in config");
    cli("http://127.0.0.1:9")
        .env("WENLAN_DATA_DIR", data.path())
        .args(["setup", "--basic"])
        .assert()
        .success()
        .stdout(predicate::str::contains("Steep (background upkeep) is off"));
    let saved: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(config_path).expect("read saved config"))
            .expect("saved config json");
    assert_eq!(saved["background_ai_enabled"], false, "{saved}");
    assert!(saved["everyday_source"].is_null(), "{saved}");
    assert!(saved["synthesis_source"].is_null(), "{saved}");
}

#[test]
fn configure_cancel_keeps_settings_unchanged_even_when_already_enabled() {
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":true}"#),
        response(&routing("pinned", "pinned")),
    ]);
    cli(&base)
        .args([
            "steep",
            "configure",
            "--everyday",
            "anthropic",
            "--synthesis",
            "on-device",
        ])
        .write_stdin("n\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Steep settings were not changed"))
        .stdout(predicate::str::contains("remains off").not());
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests
        .recv()
        .unwrap()
        .starts_with("GET /api/config/routing "));
}

#[test]
fn disable_verification_connection_failure_does_not_fall_back_to_disk() {
    let data = tempfile::tempdir().expect("consent data dir");
    let config_path = data.path().join("config.json");
    let original = r#"{"background_ai_enabled":true,"everyday_source":"anthropic","synthesis_source":"on_device"}"#;
    std::fs::write(&config_path, original).expect("seed local config");
    // The stub closes after acknowledging PUT, so the verification GET cannot
    // connect. A started live mutation must never be replaced by a disk write.
    let (base, requests) = spawn_stub(vec![
        response(r#"{"background_ai_enabled":true}"#),
        response("{}"),
    ]);
    cli(&base)
        .env("WENLAN_DATA_DIR", data.path())
        .args(["steep", "disable"])
        .assert()
        .failure()
        .stdout(predicate::str::contains("Steep disabled").not());
    assert!(requests.recv().unwrap().starts_with("GET /api/config "));
    assert!(requests.recv().unwrap().starts_with("PUT /api/config "));
    assert_eq!(
        std::fs::read_to_string(config_path).expect("read local config"),
        original
    );
}
