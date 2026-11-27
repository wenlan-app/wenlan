// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::client::WenlanClient;
use wiremock::matchers::{method, path};
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

fn add(text: &str, every_space: Option<bool>) -> AddRuleParams {
    AddRuleParams {
        text: text.to_string(),
        person_asked: "save this as a rule: we release on Tuesdays".to_string(),
        space: None,
        every_space,
        kind: None,
    }
}

fn saved() -> serde_json::Value {
    serde_json::json!({
        "status": "saved",
        "rule_id": "rule0123456789abcdef",
        "space_name": "Clinic",
        "page_id": "page_1",
        "page_title": "Rules I saved — Clinic",
        "saved_from": "agent",
        "message": "Saved. Your AI tools are told this rule from now on.",
    })
}

async fn bodies(mock: &MockServer) -> Vec<(String, serde_json::Value)> {
    mock.received_requests()
        .await
        .unwrap_or_default()
        .iter()
        .map(|request| {
            (
                format!("{} {}", request.method, request.url.path()),
                serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null),
            )
        })
        .collect()
}

/// UX-CLI-1: `add_rule` and `wenlan rules add` send the daemon the same
/// request on the same route. The CLI's twin of this test
/// (`tests/rules_cli.rs`) expects the same body, apart from the words asked
/// and where the Space travels (this side: in the body).
#[tokio::test]
async fn ux_cli_1_rules_add_matches_add_rule() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/skills/rules"))
        .respond_with(ResponseTemplate::new(200).set_body_json(saved()))
        .mount(&mock)
        .await;
    let result = crate::lock_state::with_session_pin(
        Some("Clinic".to_string()),
        server(mock.uri(), TransportMode::Stdio)
            .add_rule_impl(add("We release on Tuesdays.", None)),
    )
    .await
    .unwrap();
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert_eq!(result.structured_content.expect("a receipt"), saved());
    assert_eq!(
        bodies(&mock).await,
        vec![(
            "POST /api/skills/rules".to_string(),
            serde_json::json!({
                "text": "We release on Tuesdays.",
                "space": "Clinic",
                "person_asked": "save this as a rule: we release on Tuesdays",
            })
        )]
    );
}

/// UX-SKILL-14: a rule is for every Space only when the call says so, and
/// then no Space rides along.
#[test]
fn ux_skill_14_every_space_is_only_ever_explicit() {
    let one = WenlanMcpServer::save_rule_request(add("x", None), Some("Clinic".into()));
    assert!(!one.every_space);
    assert_eq!(one.space.as_deref(), Some("Clinic"));
    let none = WenlanMcpServer::save_rule_request(add("x", Some(false)), None);
    assert!(!none.every_space);
    assert_eq!(none.space, None);
    let every = WenlanMcpServer::save_rule_request(add("x", Some(true)), Some("Clinic".into()));
    assert!(every.every_space);
    assert_eq!(every.space, None);
}

/// UX-SKILL-14: the person's own words are required, and without them
/// nothing is sent to the daemon.
#[tokio::test]
async fn ux_skill_14_a_save_or_removal_without_the_persons_words_is_not_sent() {
    let mock = MockServer::start().await;
    let server = server(mock.uri(), TransportMode::Stdio);
    let mut params = add("We release on Tuesdays.", None);
    params.person_asked = "  ".to_string();
    assert!(server.add_rule_impl(params).await.is_err());
    assert!(server
        .remove_rule_impl(RemoveRuleParams {
            rule_id: "rule0123456789abcdef".into(),
            person_asked: String::new(),
        })
        .await
        .is_err());
    assert!(server
        .remove_rule_impl(RemoveRuleParams {
            rule_id: "../pages".into(),
            person_asked: "remove that rule".into(),
        })
        .await
        .is_err());
    assert!(bodies(&mock).await.is_empty());
}

/// UX-SKILL-14: a connection limited to one Space cannot save a rule for
/// every Space.
#[tokio::test]
async fn ux_skill_14_a_connection_limited_to_one_space_cannot_save_for_every_space() {
    let mock = MockServer::start().await;
    let result = crate::lock_state::with_session_pin(
        Some("Clinic".to_string()),
        server(mock.uri(), TransportMode::Stdio).add_rule_impl(add("x", Some(true))),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    assert!(text_of(&result).contains("Nothing was saved"));
    assert!(bodies(&mock).await.is_empty());
}

/// UX-SKILL-14: the daemon's reason for refusing reaches the AI tool, so it
/// can tell the person what to do.
#[tokio::test]
async fn ux_skill_14_a_refusal_reason_reaches_the_ai_tool() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/skills/rules"))
        .respond_with(ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": "rule_save_refused: say which Space this rule is for, or ask for every Space; nothing was saved"
        })))
        .mount(&mock)
        .await;
    let result = crate::lock_state::with_session_pin(
        Some("Clinic".to_string()),
        server(mock.uri(), TransportMode::Stdio).add_rule_impl(add("x", None)),
    )
    .await
    .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(text.contains("rule_save_refused"), "{text}");
    assert!(text.contains("say which Space this rule is for"), "{text}");
}

/// UX-SKILL-14: removing is one call to the remove route with the person's
/// words.
#[tokio::test]
async fn ux_skill_14_remove_rule_asks_the_daemon_to_remove_that_rule() {
    let mock = MockServer::start().await;
    let removed = serde_json::json!({
        "status": "removed",
        "id": "rule0123456789abcdef",
        "page_id": "page_1",
        "page_title": "Rules I saved — Clinic",
        "message": "Removed. Your AI tools are no longer told this rule.",
    });
    Mock::given(method("POST"))
        .and(path("/api/skills/rules/rule0123456789abcdef/remove"))
        .respond_with(ResponseTemplate::new(200).set_body_json(removed.clone()))
        .mount(&mock)
        .await;
    let result = server(mock.uri(), TransportMode::Stdio)
        .remove_rule_impl(RemoveRuleParams {
            rule_id: "rule0123456789abcdef".into(),
            person_asked: "drop the Tuesday rule".into(),
        })
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true), "{}", text_of(&result));
    assert_eq!(result.structured_content.expect("a receipt"), removed);
    assert_eq!(
        bodies(&mock).await,
        vec![(
            "POST /api/skills/rules/rule0123456789abcdef/remove".to_string(),
            serde_json::json!({"person_asked": "drop the Tuesday rule"})
        )]
    );
}

/// UX-SKILL-14: saving and removing are for this computer's own tools; a
/// remote connection is refused before the daemon is asked.
#[tokio::test]
async fn ux_skill_14_a_remote_connection_cannot_save_or_remove_a_rule() {
    let mock = MockServer::start().await;
    let remote = server(mock.uri(), TransportMode::Http);
    let saved = remote.add_rule_impl(add("x", None)).await.unwrap();
    assert_eq!(saved.is_error, Some(true));
    let removed = remote
        .remove_rule_impl(RemoveRuleParams {
            rule_id: "rule0123456789abcdef".into(),
            person_asked: "drop it".into(),
        })
        .await
        .unwrap();
    assert_eq!(removed.is_error, Some(true));
    assert!(bodies(&mock).await.is_empty());
}

/// UX-SKILL-14: the tool descriptions tell the AI tool when it may call
/// them, and that the person's words go along.
#[test]
fn ux_skill_14_the_tools_say_only_when_the_person_asked() {
    let tools = server("http://127.0.0.1:1".into(), TransportMode::Stdio)
        .tool_router
        .list_all();
    for name in ["add_rule", "remove_rule"] {
        let tool = tools
            .iter()
            .find(|tool| tool.name == name)
            .unwrap_or_else(|| panic!("{name} is listed"));
        let description = tool.description.as_deref().unwrap_or_default();
        assert!(description.contains("Call ONLY when the user explicitly asked"));
        assert!(description.contains("in this conversation"));
        assert!(description.contains("person_asked"));
        assert!(description.contains("One rule per call"));
        assert!(description.contains("tell the user what was"));
    }
}

/// UX-SKILL-14: `add_rule` says a rule saved for a Space reaches its connected
/// repositories as a project rule, and team rules come from the Rules page.
#[test]
fn ux_skill_14_add_rule_says_where_a_saved_rule_goes() {
    let tools = server("http://127.0.0.1:1".into(), TransportMode::Stdio)
        .tool_router
        .list_all();
    let description = tools
        .iter()
        .find(|tool| tool.name == "add_rule")
        .and_then(|tool| tool.description.as_deref())
        .unwrap_or_default();
    for said in [
        "Save one of the user's own working rules.",
        "goes into AGENTS.md, as a project rule, in each repository linked to that Space",
        "unless the Space shares its rules with a team: then the team's approved rules go there instead",
        "Team rules come only from the Rules page",
        "a proposal approved there",
    ] {
        assert!(description.contains(said), "{said}: {description}");
    }
    // Never says it is not a project rule: in the repository it is one.
    assert!(
        !description.contains("not a team or project rule"),
        "{description}"
    );
}

/// UX-SKILL-14: an AI tool reading the rules is told a saved rule was
/// saved, and from where, never that the person wrote it.
#[test]
fn ux_skill_14_read_space_rules_says_where_a_rule_was_saved_from() {
    let line = wenlan_types::SkillLine {
        id: "rule0123456789abcdef".into(),
        kind: wenlan_types::SkillLineKind::Rule,
        text: "We release on Tuesdays.".into(),
        source_page_id: "page_1".into(),
        source_page_title: "Rules I saved — Clinic".into(),
        source_revision: 2,
        source_revision_at: 1_794_000_000,
        source_quote: "We release on Tuesdays.".into(),
        person_edited: false,
        general: false,
        hand_written: false,
        saved_from: Some("Codex".into()),
        saved_by: None,
    };
    let projected = crate::query_output::project_space_rules(&wenlan_types::SpaceRulesResponse {
        space_id: "space_1".into(),
        space_name: "Clinic".into(),
        rules_page_id: None,
        saved_rules_page_ids: Vec::new(),
        lines: vec![line],
    });
    assert_eq!(projected.lines[0].source, "saved");
    assert_eq!(projected.lines[0].saved_from.as_deref(), Some("Codex"));
}
