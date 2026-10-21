//! Synthetic reviewer cases through the real DB, daemon HTTP router and MCP wrappers.
//! This is not OAuth, public hosting, or a ChatGPT/Codex client acceptance receipt.

use std::{path::Path, sync::Arc, time::Duration};

use rmcp::model::{CallToolResult, ContentBlock};
use serde_json::{json, Value};
use tokio::{sync::RwLock, task::JoinHandle};
use wenlan_core::{db::MemoryDB, NoopEmitter};
use wenlan_mcp::{
    client::WenlanClient,
    tools::{BriefParams, RecallParams, ToolProfile, TransportMode, WenlanMcpServer},
};
use wenlan_server::{router::build_router, state::ServerState};
#[path = "../examples/support/reviewer_seed.rs"]
mod reviewer_seed;
use reviewer_seed::{seed, AGENT, OTHER, SPACE};
const SECRET: &str = "UNAUTHORIZED_SENTINEL";
const TOKEN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

struct DaemonTask(JoinHandle<()>);

impl Drop for DaemonTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

fn output(result: CallToolResult) -> Value {
    assert_eq!(result.is_error, Some(false), "{result:?}");
    let structured = result.structured_content.unwrap();
    assert_eq!(result.content.len(), 1);
    let ContentBlock::Text(text) = &result.content[0] else {
        panic!("expected JSON text");
    };
    assert_eq!(
        serde_json::from_str::<Value>(&text.text).unwrap(),
        structured
    );
    for forbidden in [SECRET, "mem_private-sentinel", "mem_atlas-unavailable"] {
        assert!(!text.text.contains(forbidden), "leaked {forbidden}");
    }
    structured
}

async fn rpc(
    client: &reqwest::Client,
    url: &str,
    session: Option<&str>,
    body: Value,
) -> (Option<String>, Value) {
    let mut request = client
        .post(format!("{url}/mcp"))
        .bearer_auth(TOKEN)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2025-06-18")
        .json(&body);
    if let Some(session) = session {
        request = request.header("mcp-session-id", session);
    }
    let response = request.send().await.unwrap();
    assert_eq!(response.status(), 200);
    let session = response
        .headers()
        .get("mcp-session-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let text = response.text().await.unwrap();
    let value = serde_json::from_str::<Value>(&text).unwrap_or_else(|_| {
        // rmcp emits one complete JSON-RPC message per SSE data line.
        text.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str::<Value>(data.trim()).ok())
            .find(|message| message["id"] == body["id"])
            .expect("expected matching JSON-RPC response")
    });
    assert_eq!(value["id"], body["id"]);
    assert!(value.get("error").is_none(), "{value}");
    (session, value["result"].clone())
}

async fn verify_http(origin_url: String) {
    let port = portpicker::pick_unused_port().unwrap();
    let config = wenlan_mcp::serve::ServeConfig {
        port,
        host: "127.0.0.1".into(),
        origin_url,
        token: Some(TOKEN.into()),
        agent_name: AGENT.into(),
        user_id: None,
        allowed_origins: vec![],
        whole_library: false,
    };
    let mut task = DaemonTask(tokio::spawn(async move {
        wenlan_mcp::serve::run_serve_with_profile(config, ToolProfile::QueryOnly)
            .await
            .unwrap();
    }));
    let url = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert!(!task.0.is_finished(), "MCP server exited before readiness");
            if client
                .get(format!("{url}/health"))
                .send()
                .await
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        client
            .post(format!("{url}/mcp"))
            .json(&json!({}))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let (session, initialized) = rpc(
        &client,
        &url,
        None,
        json!({
            "jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                "protocolVersion":"2025-06-18", "capabilities":{},
                "clientInfo":{"name":"synthetic-reviewer", "version":"1"}
            }
        }),
    )
    .await;
    assert!(initialized["serverInfo"]["name"].is_string());
    let session = session.expect("stateful MCP session");
    let notified = client
        .post(format!("{url}/mcp"))
        .bearer_auth(TOKEN)
        .header("accept", "application/json, text/event-stream")
        .header("mcp-session-id", &session)
        .header("mcp-protocol-version", "2025-06-18")
        .json(&json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .send()
        .await
        .unwrap();
    assert_eq!(notified.status(), 202);
    let (_, listed) = rpc(
        &client,
        &url,
        Some(&session),
        json!({
            "jsonrpc":"2.0", "id":2, "method":"tools/list", "params":{}
        }),
    )
    .await;
    let mut names: Vec<_> = listed["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["brief", "get_page_sources", "recall"]);
    for (index, (name, arguments)) in [
        ("brief", json!({})),
        ("brief", json!({"topic":"Atlas authentication decision"})),
        (
            "recall",
            json!({"query":"Atlas authentication decision", "limit":3, "rerank":false}),
        ),
        ("get_page_sources", json!({"page_id":"page_atlas-auth"})),
        (
            "get_page_sources",
            json!({"page_id":"page_atlas-unavailable"}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let (_, result) = rpc(
            &client,
            &url,
            Some(&session),
            json!({
                "jsonrpc":"2.0", "id":index+3, "method":"tools/call",
                "params":{"name":name,"arguments":arguments}
            }),
        )
        .await;
        let result = output(serde_json::from_value(result).unwrap());
        match index {
            0 => assert_eq!(result["brief"]["active"][0]["text"], "Use signed requests"),
            1 => assert!(result["related_context"]["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["source_id"] == "mem_atlas-auth")),
            2 => assert!(result["results"]
                .as_array()
                .unwrap()
                .iter()
                .any(|hit| hit["source_id"] == "mem_atlas-auth")),
            3 => assert_eq!(result["sources"].as_array().unwrap().len(), 1),
            4 => assert_eq!(
                result,
                json!({"page_id":"page_atlas-unavailable","sources":[]})
            ),
            _ => unreachable!(),
        }
    }
    let deleted = client
        .delete(format!("{url}/mcp"))
        .bearer_auth(TOKEN)
        .header("mcp-session-id", &session)
        .send()
        .await
        .unwrap();
    assert_eq!(deleted.status(), 202);
    if std::env::var("WENLAN_TEST_REAL_RELAY").as_deref() == Ok("1") {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = tokio::process::Command::new("node");
        command
            .current_dir(root)
            .arg("relay/tests/real-backend-check.mjs")
            .env("WENLAN_TEST_MCP_URL", &url)
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(50), command.output())
            .await
            .expect("relay integration deadline")
            .expect("launch Node relay test");
        assert!(
            output.status.success(),
            "relay test failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("{}", String::from_utf8_lossy(&output.stdout));
    }
    task.0.abort();
    assert!((&mut task.0).await.unwrap_err().is_cancelled());
    assert!(tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .is_err());
}

async fn verify(root: &Path, preseeded: bool) {
    let db = Arc::new(
        MemoryDB::new(&root.join("memorydb"), Arc::new(NoopEmitter))
            .await
            .unwrap(),
    );
    if !preseeded {
        seed(&db, &root.join("pages")).await;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = ServerState {
        db: Some(db.clone()),
        bound_port: addr.port(),
        brief_status_root: Some(root.join("status")),
        ..Default::default()
    };
    let router = build_router(Arc::new(RwLock::new(state)));
    let mut daemon = DaemonTask(tokio::spawn(async move {
        axum::serve(listener, router.into_make_service())
            .await
            .unwrap();
    }));
    let client = WenlanClient::new(format!("http://{addr}")).with_agent_name(AGENT.into());
    let mcp = WenlanMcpServer::new(client, TransportMode::Http, AGENT.into(), None)
        .with_tool_profile(ToolProfile::QueryOnly);

    let brief = output(
        mcp.brief_impl(BriefParams {
            session: None,
            topic: None,
            space: None,
        })
        .await
        .unwrap(),
    );
    assert_eq!(brief["state"], "ready");
    assert_eq!(brief["space"], SPACE);
    assert_eq!(brief["brief"]["active"][0]["text"], "Use signed requests");
    assert_eq!(
        brief["brief"]["backlog"][0]["text"],
        "Document offline fallback"
    );
    assert_eq!(
        brief["brief"]["last_session_summary"],
        "Atlas uses signed requests."
    );
    assert!(brief["related_context"].is_null());

    let related = output(
        mcp.brief_impl(BriefParams {
            session: None,
            topic: Some("Atlas authentication decision".into()),
            space: None,
        })
        .await
        .unwrap(),
    );
    assert_eq!(related["brief"], brief["brief"]);
    assert!(related["related_context"]["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["source_id"] == "mem_atlas-auth"));

    let searches_before = db
        .list_agent_activity(100, Some(AGENT), None)
        .await
        .unwrap()
        .iter()
        .filter(|entry| entry.action == "search")
        .count();
    let recalled = output(
        mcp.recall_impl(RecallParams {
            session: None,
            query: "Atlas authentication decision".into(),
            limit: Some(3),
            memory_type: None,
            space: None,
            rerank: Some(false),
        })
        .await
        .unwrap(),
    );
    assert!(recalled["results"]
        .as_array()
        .unwrap()
        .iter()
        .any(|hit| hit["source_id"] == "mem_atlas-auth"));
    assert!(recalled["supplemental_pages"].is_array());
    assert_eq!(
        db.list_agent_activity(100, Some(AGENT), None)
            .await
            .unwrap()
            .iter()
            .filter(|entry| entry.action == "search")
            .count(),
        searches_before + 1
    );

    let sources = output(mcp.get_page_sources_impl("page_atlas-auth").await.unwrap());
    assert_eq!(sources["page_id"], "page_atlas-auth");
    assert_eq!(sources["sources"].as_array().unwrap().len(), 1);
    assert!(sources.to_string().contains("mem_atlas-auth"));
    assert!(!sources
        .to_string()
        .contains(&root.to_string_lossy().to_string()));

    let unavailable = output(
        mcp.get_page_sources_impl("page_atlas-unavailable")
            .await
            .unwrap(),
    );
    assert_eq!(
        unavailable,
        json!({"page_id": "page_atlas-unavailable", "sources": []})
    );
    let denied = mcp
        .get_page_sources_impl("page_private-sentinel")
        .await
        .unwrap();
    assert_eq!(denied.is_error, Some(true));
    assert!(!serde_json::to_string(&denied).unwrap().contains(SECRET));

    // A model-supplied Space must not escape the connector's process pin.
    let pinned = output(
        mcp.brief_impl(BriefParams {
            session: None,
            topic: None,
            space: Some(OTHER.into()),
        })
        .await
        .unwrap(),
    );
    assert_eq!(pinned, brief);

    verify_http(format!("http://{addr}")).await;

    daemon.0.abort();
    let result = (&mut daemon.0).await;
    assert!(result.unwrap_err().is_cancelled());
    assert!(tokio::net::TcpStream::connect(addr).await.is_err());
}

#[test]
fn reviewer_cases_use_real_scoped_backend() {
    // This integration binary has one test. Set process configuration before
    // creating Tokio threads; never mutate another test's or user's runtime.
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path().canonicalize().unwrap().join("library");
    let preseeded = if let Some(binary) = std::env::var_os("WENLAN_TEST_REVIEWER_SEED_BIN") {
        let result = std::process::Command::new(&binary)
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "seed failed: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let config_bytes = std::fs::read(root.join("config.json")).unwrap();
        let config: Value = serde_json::from_slice(&config_bytes).unwrap();
        assert_eq!(
            config["knowledge_path"],
            root.join("pages").to_string_lossy().as_ref()
        );
        assert_eq!(config["sources"], json!([]));
        assert!(root.join("memorydb/origin_memory.db").is_file());
        let duplicate = std::process::Command::new(binary)
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            !duplicate.status.success(),
            "must not overwrite an existing library"
        );
        assert_eq!(
            std::fs::read(root.join("config.json")).unwrap(),
            config_bytes
        );
        true
    } else {
        std::fs::create_dir(&root).unwrap();
        false
    };
    std::env::set_var("WENLAN_NO_AUTOSTART", "1");
    std::env::set_var("WENLAN_DATA_DIR", &root);
    std::env::set_var("HOME", root.join("home"));
    std::env::set_var("USERPROFILE", root.join("home"));
    std::env::set_var("WENLAN_SPACE", SPACE);
    wenlan_mcp::lock_state::init_from_env();
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        tokio::time::timeout(Duration::from_secs(60), verify(&root, preseeded))
            .await
            .unwrap();
    });
    drop(runtime);
    scratch.close().unwrap();
}
