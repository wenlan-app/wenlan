// SPDX-License-Identifier: Apache-2.0
//! The `theme` tool: an AI tool asked to "make Wenlan purple" can save and
//! apply a theme in one call, without knowing the file format or folder.
use super::*;
use crate::client::WenlanClient;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn server(uri: String) -> WenlanMcpServer {
    WenlanMcpServer::new(
        WenlanClient::new(uri),
        TransportMode::Stdio,
        "claude-code".into(),
        None,
    )
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

async fn requests(mock: &MockServer) -> Vec<(String, serde_json::Value)> {
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

fn purple() -> serde_json::Value {
    serde_json::json!({
        "name": "Purple",
        "description": "A purple look",
        "colors": {"background": "#2A1B3D", "accent": "#7C3AED"}
    })
}

#[test]
fn a_tool_only_describes_the_look() {
    let (theme, ignored) = theme_file_from_tool(purple(), "Claude Code").unwrap();
    assert_eq!(theme.schema, wenlan_types::theme::THEME_SCHEMA_VERSION);
    assert_eq!(theme.base, wenlan_types::theme::ThemeBase::Dark);
    assert_eq!(theme.author.as_deref(), Some("Claude Code"));
    assert_eq!(ignored, vec!["description".to_string()]);
    // What the tool did say is kept as it said it.
    let (theme, _) = theme_file_from_tool(
        serde_json::json!({"name": "Day", "base": "light", "author": "Qi-Xuan"}),
        "Claude Code",
    )
    .unwrap();
    assert_eq!(theme.base, wenlan_types::theme::ThemeBase::Light);
    assert_eq!(theme.author.as_deref(), Some("Qi-Xuan"));
    assert!(theme_file_from_tool(serde_json::json!("purple"), "").is_err());
}

#[test]
fn theme_authors_read_as_the_tools_name() {
    assert_eq!(theme_author("claude-code"), "Claude Code");
    assert_eq!(theme_author("codex-mcp-client"), "Codex");
    assert_eq!(theme_author("claude-code@team.ana"), "Claude Code");
    assert_eq!(theme_author("unknown"), "");
}

/// The example the tool shows passes Wenlan's own checks, so copying it
/// works on the first try.
#[test]
fn the_example_passes_every_check() {
    let json = THEME_EXAMPLE
        .split_once("theme=")
        .map(|(_, rest)| rest.trim_end_matches(')'))
        .unwrap();
    let value: serde_json::Value = serde_json::from_str(json).unwrap();
    let (theme, ignored) = theme_file_from_tool(value, "Claude Code").unwrap();
    assert!(ignored.is_empty());
    let tmp = tempfile::tempdir().unwrap();
    let (problems, _) = wenlan_core::theme::check_unsaved(tmp.path(), "purple", &theme);
    assert_eq!(problems, vec![]);
    // Nothing was moved either: the example is already readable as written.
    assert_eq!(wenlan_core::theme::complete_theme(&theme), theme);
}

#[tokio::test]
async fn apply_with_a_theme_saves_and_applies_it_in_one_call() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/themes/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"id": "purple", "ok": true, "problems": [], "tokens": {}}),
        ))
        .mount(&mock)
        .await;
    let mut kept = purple();
    kept.as_object_mut().unwrap().remove("description");
    kept["schema"] = serde_json::json!(1);
    kept["base"] = serde_json::json!("dark");
    kept["author"] = serde_json::json!("Claude Code");
    kept["colors"]["accent"] = serde_json::json!("#8344EE");
    kept["colors"]["textOnAccent"] = serde_json::json!("#E7E7E8");
    Mock::given(method("PUT"))
        .and(path("/api/themes/purple"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": "purple", "theme": kept, "problems": []})),
        )
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/themes/active"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "purple", "name": "Purple", "base": "dark", "revision": "r1",
            "tokens": {}, "problems": []
        })))
        .mount(&mock)
        .await;

    let result = server(mock.uri())
        .theme_impl(ThemeParams {
            action: ThemeAction::Apply,
            id: Some("purple".into()),
            theme: Some(purple()),
        })
        .await
        .unwrap();
    let text = text_of(&result);
    assert_ne!(result.is_error, Some(true), "{text}");
    assert!(text.contains("Saved theme purple."), "{text}");
    assert!(text.contains("description"), "ignored field named: {text}");
    assert!(text.contains("accent #7C3AED -> #8344EE"), "{text}");
    assert!(text.contains("Theme purple is in use"), "{text}");

    let sent = requests(&mock).await;
    let routes: Vec<&str> = sent.iter().map(|(route, _)| route.as_str()).collect();
    assert_eq!(
        routes,
        [
            "POST /api/themes/check",
            "PUT /api/themes/purple",
            "POST /api/themes/check",
            "PUT /api/themes/active",
        ]
    );
    let checked = &sent[0].1;
    assert_eq!(checked["theme"]["schema"], 1);
    assert_eq!(checked["theme"]["author"], "Claude Code");
    assert!(checked["theme"].get("description").is_none());
}

#[tokio::test]
async fn a_refused_theme_is_not_saved_and_every_problem_comes_back() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/themes/check"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "fog", "ok": false, "tokens": {},
            "problems": [{"field": "colors.text", "message": "text on background has contrast 1.8:1; it needs at least 4.5:1 to stay readable. Try #C9C3D0 for text."}]
        })))
        .mount(&mock)
        .await;
    let result = server(mock.uri())
        .theme_impl(ThemeParams {
            action: ThemeAction::Save,
            id: Some("fog".into()),
            theme: Some(serde_json::json!({"name": "Fog", "colors": {"text": "#5A4A6A"}})),
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(text.contains("was not saved"), "{text}");
    assert!(text.contains("Try #C9C3D0"), "{text}");
    let routes: Vec<String> = requests(&mock).await.into_iter().map(|(r, _)| r).collect();
    assert_eq!(routes, ["POST /api/themes/check"]);
}

#[tokio::test]
async fn an_empty_list_says_the_built_in_look_is_in_use_and_how_to_make_one() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/themes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "themes_dir": "/tmp/themes", "active": null, "themes": [], "allowed_fonts": ["Fraunces"]
        })))
        .mount(&mock)
        .await;
    let result = server(mock.uri())
        .theme_impl(ThemeParams {
            action: ThemeAction::List,
            id: None,
            theme: None,
        })
        .await
        .unwrap();
    let text = text_of(&result);
    assert!(text.contains("No custom themes yet"), "{text}");
    assert!(
        text.contains(r#"theme(action="save", id="purple""#),
        "{text}"
    );
}
