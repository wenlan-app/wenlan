// SPDX-License-Identifier: Apache-2.0

use rmcp::model::{CallToolResult, ContentBlock};
use wenlan_mcp::{
    client::WenlanClient,
    tools::{BriefParams, TransportMode, WenlanMcpServer},
};
use wenlan_types::{
    Brief, BriefItem, BriefItemState, BriefReadResponse, BriefReadState, BriefRelatedContext,
};
use wiremock::{
    matchers::{body_json, method, path},
    Mock, MockServer, ResponseTemplate,
};

fn make_server(mock: &MockServer) -> WenlanMcpServer {
    // A session exported by the developer's own agent would add a
    // `POST /api/sessions` request these tests do not expect.
    std::env::remove_var("WENLAN_SESSION");
    std::env::remove_var("CODEX_THREAD_ID");
    WenlanMcpServer::new(
        WenlanClient::new(mock.uri()),
        TransportMode::Stdio,
        "test-agent".into(),
        None,
    )
}

fn text_of(result: &CallToolResult) -> &str {
    result
        .content
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .expect("expected text content")
}

fn ready_response(topic: Option<&str>) -> BriefReadResponse {
    BriefReadResponse {
        state: BriefReadState::Ready,
        space: Some("wenlan".into()),
        brief: Some(Brief {
            space_id: "space-1".into(),
            space: "wenlan".into(),
            last_session_summary: "MCP work is next.".into(),
            last_handoff_at: Some(1_785_196_800),
            version: 7,
            active: vec![BriefItem {
                id: "item-1".into(),
                text: "Replace the public context tool.".into(),
                state: BriefItemState::Active,
                added_at: 1_785_110_400,
                gate: Some("MCP tests pass".into()),
                version: 3,
            }],
            backlog: Vec::new(),
        }),
        related_context: topic.map(|query| BriefRelatedContext {
            query: query.into(),
            results: Vec::new(),
        }),
        code_basis_pending: None,
        code_basis_pending_explained: false,
        getting_started: None,
        instructions: None,
    }
}

#[tokio::test]
async fn no_topic_reads_only_the_complete_brief_and_preserves_versions() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .and(body_json(serde_json::json!({
            "topic": null,
            "space": "wenlan"
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(ready_response(None)))
        .mount(&mock)
        .await;

    let result = make_server(&mock)
        .brief_impl(BriefParams {
            session: None,
            topic: None,
            space: Some("wenlan".into()),
        })
        .await
        .expect("brief read succeeds");
    let text = text_of(&result);

    assert!(text.starts_with("Space Brief: wenlan (v7)\n"), "{text}");
    assert!(text.contains("Last session: MCP work is next.\n"), "{text}");
    assert!(
        text.contains("Active (1):\n  - item-1 (v3): Replace the public context tool. [gate: MCP tests pass]\n"),
        "{text}"
    );
    assert!(text.contains("Backlog: none\n"), "{text}");
    assert!(!text.contains("Related Context"));
}

#[tokio::test]
async fn topic_appends_separately_labeled_same_space_context() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .and(body_json(serde_json::json!({
            "topic": "release readiness",
            "space": "wenlan"
        })))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(ready_response(Some("release readiness"))),
        )
        .mount(&mock)
        .await;

    let result = make_server(&mock)
        .brief_impl(BriefParams {
            session: None,
            topic: Some("release readiness".into()),
            space: Some("wenlan".into()),
        })
        .await
        .expect("brief topic read succeeds");
    let text = text_of(&result);

    assert!(text.contains("Space Brief:"));
    assert!(text.contains("Related Context (query: release readiness):"));
}

#[tokio::test]
async fn brief_says_how_many_notes_rely_on_code_that_changed() {
    for (pending, expected) in [
        (
            Some(3_u32),
            Some("3 notes rely on code that changed; /wenlan steep brings them up to date (Codex: $wenlan steep)"),
        ),
        (
            Some(1),
            Some("1 note relies on code that changed; /wenlan steep brings them up to date (Codex: $wenlan steep)"),
        ),
        (Some(0), None),
        (None, None),
    ] {
        let mock = MockServer::start().await;
        let mut response = ready_response(None);
        response.code_basis_pending = pending;
        Mock::given(method("POST"))
            .and(path("/api/brief"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&mock)
            .await;
        let result = make_server(&mock)
            .brief_impl(BriefParams {
                session: None,
                topic: None,
                space: Some("wenlan".into()),
            })
            .await
            .expect("brief read succeeds");
        let text = text_of(&result);
        match expected {
            Some(line) => assert!(text.contains(line), "{text}"),
            None => {
                assert!(!text.contains("rely on code that changed"), "{text}");
                assert!(!text.contains("/wenlan steep brings"), "{text}");
            }
        }
    }

    // A Space with no Brief yet still reports drift.
    for (pending, expected) in [
        (
            Some(2_u32),
            Some(
                "2 notes rely on code that changed; /wenlan steep brings them up to date (Codex: $wenlan steep)",
            ),
        ),
        (Some(0), None),
        (None, None),
    ] {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/brief"))
            .respond_with(ResponseTemplate::new(200).set_body_json(BriefReadResponse {
                state: BriefReadState::BriefNotCreated,
                space: Some("wenlan".into()),
                brief: None,
                related_context: None,
                code_basis_pending: pending,
                code_basis_pending_explained: false,
                getting_started: None,
                instructions: None,
            }))
            .mount(&mock)
            .await;
        let result = make_server(&mock)
            .brief_impl(BriefParams {
                topic: None,
                space: Some("wenlan".into()),
                session: None,
            })
            .await
            .expect("missing brief is a successful read state");
        let text = text_of(&result);
        assert!(
            text.starts_with("No Brief exists for Space `wenlan` yet."),
            "{text}"
        );
        match expected {
            Some(line) => assert!(text.contains(line), "{text}"),
            None => assert!(!text.contains("rely on code that changed"), "{text}"),
        }
    }
}

#[tokio::test]
async fn brief_says_when_the_model_has_already_looked_at_every_changed_note() {
    for (explained, expect_suffix) in [(true, true), (false, false)] {
        let mock = MockServer::start().await;
        let mut response = ready_response(None);
        response.code_basis_pending = Some(2);
        response.code_basis_pending_explained = explained;
        Mock::given(method("POST"))
            .and(path("/api/brief"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response))
            .mount(&mock)
            .await;
        let result = make_server(&mock)
            .brief_impl(BriefParams {
                session: None,
                topic: None,
                space: Some("wenlan".into()),
            })
            .await
            .expect("brief read succeeds");
        let text = text_of(&result);
        assert!(text.contains("2 notes rely on code that changed"), "{text}");
        assert_eq!(
            text.contains("AI has looked at them; /wenlan steep fixes the obvious ones and the rest need a person (Codex: $wenlan steep)."),
            expect_suffix,
            "{text}"
        );
    }

    // The flag never shows when nothing is pending.
    let mock = MockServer::start().await;
    let mut response = ready_response(None);
    response.code_basis_pending = Some(0);
    response.code_basis_pending_explained = true;
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&mock)
        .await;
    let result = make_server(&mock)
        .brief_impl(BriefParams {
            session: None,
            topic: None,
            space: Some("wenlan".into()),
        })
        .await
        .expect("brief read succeeds");
    assert!(
        !text_of(&result).contains("AI has looked"),
        "{}",
        text_of(&result)
    );
}

#[tokio::test]
async fn missing_brief_explains_that_reads_do_not_create_it() {
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .respond_with(ResponseTemplate::new(200).set_body_json(BriefReadResponse {
            state: BriefReadState::BriefNotCreated,
            space: Some("wenlan".into()),
            brief: None,
            related_context: None,
            code_basis_pending: None,
            code_basis_pending_explained: false,
            getting_started: None,
            instructions: None,
        }))
        .mount(&mock)
        .await;

    let result = make_server(&mock)
        .brief_impl(BriefParams {
            session: None,
            topic: None,
            space: Some("wenlan".into()),
        })
        .await
        .expect("missing brief is a successful read state");

    assert_eq!(
        text_of(&result),
        "No Brief exists for Space `wenlan` yet. Reads never create one; the first handoff update can."
    );
}

#[tokio::test]
async fn brief_tells_a_new_user_the_next_step_even_without_a_project() {
    use wenlan_types::{GettingStarted, GettingStartedStep};
    let cases = [
        (
            GettingStarted { pages: 0, waiting: 12, held: 0, next: GettingStartedStep::WritePages },
            "Getting started (whole library): no pages yet, 12 items (notes, chats, files or captures) waiting to be written into pages.",
            "in Claude Code they type /wenlan write my pages, in Codex $wenlan write my pages",
        ),
        (
            GettingStarted { pages: 3, waiting: 0, held: 0, next: GettingStartedStep::Ask },
            "Getting started (whole library): 3 pages, nothing waiting.",
            "ask a question about their pages in plain words; recall answers it.",
        ),
        (
            GettingStarted { pages: 0, waiting: 0, held: 0, next: GettingStartedStep::AddMaterial },
            "Getting started (whole library): no pages yet, nothing waiting.",
            "in the Wenlan app (Add a source)",
        ),
    ];
    for (getting_started, counts, next) in cases {
        let mock = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/brief"))
            .respond_with(ResponseTemplate::new(200).set_body_json(BriefReadResponse {
                state: BriefReadState::SpaceNotResolved,
                space: None,
                brief: None,
                related_context: None,
                code_basis_pending: None,
                code_basis_pending_explained: false,
                getting_started: Some(getting_started),
                instructions: None,
            }))
            .mount(&mock)
            .await;
        let result = make_server(&mock)
            .brief_impl(BriefParams {
                session: None,
                topic: None,
                space: None,
            })
            .await
            .expect("unresolved Space is a successful read state");
        let text = text_of(&result);
        assert!(text.starts_with("Space not resolved."), "{text}");
        assert!(text.contains(counts), "{text}");
        assert!(text.contains("Next step for the user: "), "{text}");
        assert!(text.contains(next), "{text}");
    }
}

/// UX-HOW-1: the person's page on how AI should work with them comes first
/// in the Brief, with its link, and says changes are proposals.
#[tokio::test]
async fn brief_opens_with_the_instructions_page() {
    let mock = MockServer::start().await;
    let mut response = ready_response(None);
    response.instructions = Some(wenlan_types::chat_check::InstructionsPage {
        page_id: "page_how".into(),
        title: "How you want AI to work with you".into(),
        version: 2,
        content: "Answer briefly, conclusion first.\n".into(),
        role: "instructions".into(),
        link: "wenlan://page/page_how".into(),
    });
    Mock::given(method("POST"))
        .and(path("/api/brief"))
        .respond_with(ResponseTemplate::new(200).set_body_json(response))
        .mount(&mock)
        .await;

    let result = make_server(&mock)
        .brief_impl(BriefParams {
            session: None,
            topic: None,
            space: Some("wenlan".into()),
        })
        .await
        .expect("brief read succeeds");
    let text = text_of(&result);

    assert!(
        text.starts_with(
            "How the person wants AI to work with them: How you want AI to work with you (v2, wenlan://page/page_how)\n"
        ),
        "{text}"
    );
    assert!(text.contains("propose an edit"), "{text}");
    assert!(
        text.contains("Answer briefly, conclusion first.\n\nSpace Brief: wenlan (v7)"),
        "{text}"
    );
}
