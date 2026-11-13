// SPDX-License-Identifier: Apache-2.0
use super::*;
use crate::client::WenlanClient;
use wiremock::matchers::{body_partial_json, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn server(uri: String) -> WenlanMcpServer {
    WenlanMcpServer::new(
        WenlanClient::new(uri),
        TransportMode::Stdio,
        "agent".into(),
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

fn look_data_json() -> serde_json::Value {
    serde_json::json!({
        "api": 1,
        "page": {
            "id": "pg_trip", "title": "Japan trip", "space": "Travel",
            "created": "2026-11-01T08:00:00Z", "modified": "2026-11-02T09:30:00Z",
            "text": "Two weeks in Japan.", "html": "<p>Two weeks in Japan.</p>",
            "sections": [{
                "heading": "Plan", "level": 2, "text": "- [x] Book flights",
                "html": "<ul><li>Book flights</li></ul>",
                "items": [{"text": "Book flights", "done": true}], "tables": []
            }],
            "links": []
        },
        "space": {"name": "Travel", "pages": [], "more": false},
        "theme": {"mode": "dark"}
    })
}

async fn mount_page(mock: &MockServer, look: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/api/pages/pg_trip/look"))
        .respond_with(ResponseTemplate::new(200).set_body_json(look))
        .mount(mock)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/pages/pg_trip/look/data"))
        .respond_with(ResponseTemplate::new(200).set_body_json(look_data_json()))
        .mount(mock)
        .await;
}

#[tokio::test]
async fn reading_a_page_look_gives_the_real_data_the_rules_and_an_example() {
    let mock = MockServer::start().await;
    mount_page(&mock, serde_json::json!({"look": null, "versions": []})).await;
    let result = server(mock.uri())
        .read_page_look_impl(ReadPageLookParams {
            page: "pg_trip".into(),
            session: None,
        })
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(text.contains("This page has no look yet."), "{text}");
    assert!(
        text.contains("\"title\": \"Japan trip\""),
        "the real data: {text}"
    );
    assert!(text.contains("\"done\": true"));
    assert!(text.contains("Rules for a look"));
    assert!(text.contains("wenlan.open(pageId)"));
    assert!(
        text.contains("const { page } = wenlan;"),
        "the example uses wenlan.page"
    );
    assert!(text.contains("set_page_look"));
}

#[tokio::test]
async fn reading_shows_the_current_look_and_its_last_error() {
    let mock = MockServer::start().await;
    mount_page(
        &mock,
        serde_json::json!({
            "look": {"id": 4, "html": "<h1>old</h1>", "api": 1, "kept_at": 5,
                     "last_error": "ReferenceError: foo is not defined"},
            "versions": []
        }),
    )
    .await;
    let text = text_of(
        &server(mock.uri())
            .read_page_look_impl(ReadPageLookParams {
                page: "pg_trip".into(),
                session: None,
            })
            .await
            .unwrap(),
    );
    assert!(text.contains("<h1>old</h1>"), "{text}");
    assert!(text.contains("ReferenceError: foo is not defined"));
}

#[tokio::test]
async fn a_title_finds_the_page_and_a_missing_or_double_title_says_so() {
    let mock = MockServer::start().await;
    mount_page(&mock, serde_json::json!({"look": null, "versions": []})).await;
    Mock::given(method("POST"))
        .and(path("/api/pages/search"))
        .and(body_partial_json(
            serde_json::json!({"query": "Japan trip"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "pages": [
                {"id": "pg_other", "title": "Japan trip planning"},
                {"id": "pg_trip", "title": "japan TRIP"}
            ]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/search"))
        .and(body_partial_json(serde_json::json!({"query": "Twin"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "pages": [{"id": "a1", "title": "Twin"}, {"id": "b2", "title": "Twin"}]
        })))
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/pages/search"))
        .and(body_partial_json(serde_json::json!({"query": "Nothing"})))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"pages": []})))
        .mount(&mock)
        .await;
    let server = server(mock.uri());
    let read = |page: &str| {
        server.read_page_look_impl(ReadPageLookParams {
            page: page.into(),
            session: None,
        })
    };
    let found = read("Japan trip").await.unwrap();
    assert!(text_of(&found).contains("page_id pg_trip"));
    let twin = read("Twin").await.unwrap();
    assert_eq!(twin.is_error, Some(true));
    assert!(text_of(&twin).contains("a1, b2"));
    let none = read("Nothing").await.unwrap();
    assert_eq!(none.is_error, Some(true));
    assert!(text_of(&none).contains("No page is titled"));
}

#[tokio::test]
async fn setting_a_look_puts_the_html_and_says_what_the_person_sees() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages/pg_trip/look"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"look": null, "versions": []})),
        )
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/pages/pg_trip/look"))
        .and(body_partial_json(
            serde_json::json!({"html": "<h1 id=t></h1>"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"page_id": "pg_trip", "look_id": 7, "created": true}),
        ))
        .expect(1)
        .mount(&mock)
        .await;
    let result = server(mock.uri())
        .set_page_look_impl(SetPageLookParams {
            page: "pg_trip".into(),
            html: Some("<h1 id=t></h1>".into()),
            path: None,
            session: None,
        })
        .await
        .unwrap();
    assert_ne!(result.is_error, Some(true));
    assert!(text_of(&result).contains("Look | Text"));
}

#[tokio::test]
async fn a_refusal_reason_reaches_the_ai_word_for_word() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages/pg_trip/look"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"look": null, "versions": []})),
        )
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/pages/pg_trip/look"))
        .respond_with(ResponseTemplate::new(422).set_body_json(serde_json::json!({
            "error": "look_refused: The look copies the page's text into the HTML (3 whole sentences appear literally): use wenlan.page instead of copying text."
        })))
        .mount(&mock)
        .await;
    let result = server(mock.uri())
        .set_page_look_impl(SetPageLookParams {
            page: "pg_trip".into(),
            html: Some("<p>x</p>".into()),
            path: None,
            session: None,
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let text = text_of(&result);
    assert!(
        text.contains("use wenlan.page instead of copying text"),
        "{text}"
    );
    assert!(text.contains("Nothing was changed"));
}

#[tokio::test]
async fn a_look_can_be_given_as_a_file() {
    let mock = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/pages/pg_trip/look"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"look": null, "versions": []})),
        )
        .mount(&mock)
        .await;
    Mock::given(method("PUT"))
        .and(path("/api/pages/pg_trip/look"))
        .and(body_partial_json(
            serde_json::json!({"html": "<p>from file</p>"}),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"page_id": "pg_trip", "look_id": 1, "created": false}),
        ))
        .expect(1)
        .mount(&mock)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("look.html");
    std::fs::write(&file, "<p>from file</p>").unwrap();
    let server = server(mock.uri());
    let ok = server
        .set_page_look_impl(SetPageLookParams {
            page: "pg_trip".into(),
            html: None,
            path: Some(file.display().to_string()),
            session: None,
        })
        .await
        .unwrap();
    assert!(text_of(&ok).contains("already the page's look"));
    for params in [
        SetPageLookParams {
            page: "pg_trip".into(),
            html: None,
            path: Some("relative/look.html".into()),
            session: None,
        },
        SetPageLookParams {
            page: "pg_trip".into(),
            html: None,
            path: None,
            session: None,
        },
    ] {
        let refused = server.set_page_look_impl(params).await.unwrap();
        assert_eq!(refused.is_error, Some(true));
    }
}

#[test]
fn the_look_tools_stay_local_and_out_of_the_remote_profiles() {
    use crate::tools::ToolProfile;
    for profile in [ToolProfile::QueryOnly, ToolProfile::WebWrite] {
        let server = server("http://127.0.0.1:1".into()).with_tool_profile(profile);
        let names: Vec<String> = server
            .visible_tools()
            .into_iter()
            .map(|tool| tool.name.to_string())
            .collect();
        assert!(
            !names.iter().any(|n| n.contains("page_look")),
            "{profile:?}: {names:?}"
        );
    }
    let mut http = server("http://127.0.0.1:1".into());
    http.transport = TransportMode::Http;
    let names: Vec<String> = http
        .visible_tools()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(!names.iter().any(|n| n.contains("page_look")), "{names:?}");
    let stdio = server("http://127.0.0.1:1".into());
    let names: Vec<String> = stdio
        .visible_tools()
        .into_iter()
        .map(|tool| tool.name.to_string())
        .collect();
    assert!(names.contains(&"read_page_look".to_string()));
    assert!(names.contains(&"set_page_look".to_string()));
}
