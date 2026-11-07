use async_trait::async_trait;
use axum::body::to_bytes;
use axum::http::{Method, StatusCode};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;
use tower::ServiceExt;
use wenlan_core::lint::observation::LintRunEvent;
use wenlan_core::llm_provider::{LlmBackend, LlmError, LlmProvider, LlmRequest};
use wenlan_types::lint::{
    LintAgentSubmission, LintAgentVerdict, LintAgentWork, LintErrorResponse, LintMetricCode,
    LintMetricValue, LintOutcome, LintProfile, LintReasonCode, LintReport, LintScopeKind,
    LintSemanticCheckId, LintSemanticDecision,
};
use wenlan_types::sources::{Source, SourceType, SyncStatus};

#[path = "lint_endpoint_test/support.rs"]
mod support;
use support::{json_request, request, Fixture};

struct FakeApiProvider {
    calls: AtomicUsize,
}

struct BlockingApiProvider {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl LlmProvider for FakeApiProvider {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let packet: serde_json::Value = serde_json::from_str(&request.user_prompt).unwrap();
        let verdicts = packet["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|candidate| serde_json::json!([candidate["reference"], "pass", 9000, []]))
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "verdicts": verdicts }).to_string())
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "fake-api"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

#[async_trait]
impl LlmProvider for BlockingApiProvider {
    async fn generate(&self, request: LlmRequest) -> Result<String, LlmError> {
        self.entered.notify_one();
        self.release.notified().await;
        let packet: serde_json::Value = serde_json::from_str(&request.user_prompt).unwrap();
        let verdicts = packet["candidates"]
            .as_array()
            .unwrap()
            .iter()
            .map(|candidate| serde_json::json!([candidate["reference"], "pass", 9000, []]))
            .collect::<Vec<_>>();
        Ok(serde_json::json!({ "verdicts": verdicts }).to_string())
    }

    fn is_available(&self) -> bool {
        true
    }
    fn name(&self) -> &str {
        "blocking-api"
    }
    fn backend(&self) -> LlmBackend {
        LlmBackend::Api
    }
}

async fn report(fixture: &Fixture, uri: &str) -> (StatusCode, String, Option<LintReport>) {
    let response = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, uri))
        .await
        .expect("lint response");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("lint body");
    let body = String::from_utf8(bytes.to_vec()).expect("utf8 body");
    let decoded = serde_json::from_str(&body).ok();
    (status, body, decoded)
}

#[tokio::test]
async fn lint_global_complete_response_uses_shared_remote_safe_report() {
    let fixture = Fixture::new(Vec::new(), None).await;

    let (status, body, decoded) = report(&fixture, "/api/lint").await;

    let decoded = decoded.expect("shared LintReport");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decoded.scope().kind(), LintScopeKind::Global);
    assert_eq!(decoded.profile(), LintProfile::General);
    assert!(decoded.complete());
    assert_eq!(decoded.totals().incomplete(), 0);
    assert!(!body.contains(fixture.root.path().to_string_lossy().as_ref()));
    assert!(!body.contains("knowledge_path"));
}

#[tokio::test]
async fn lint_profile_is_typed_and_unknown_values_fail_before_runner_work() {
    let fixture = Fixture::new(Vec::new(), None).await;

    let (status, _, deep) = report(&fixture, "/api/lint?profile=deep").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(deep.expect("deep report").profile(), LintProfile::Deep);

    let before_events = fixture.lint_events.events();
    let (status, _, decoded) = report(&fixture, "/api/lint?profile=future").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(decoded.is_none());
    assert_eq!(fixture.lint_events.events(), before_events);
}

#[tokio::test]
async fn lint_deep_requires_explicit_external_egress_before_using_configured_api_provider() {
    let provider = Arc::new(FakeApiProvider {
        calls: AtomicUsize::new(0),
    });
    let configured: Arc<dyn LlmProvider> = provider.clone();
    let fixture = Fixture::new_with_state(Vec::new(), None, move |state| {
        state.synthesis_llm = Some(configured);
    })
    .await;
    fixture.seed_semantic_candidates().await;

    let (status, _, _) = report(&fixture, "/api/lint?profile=deep").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0);

    let (status, _, decoded) =
        report(&fixture, "/api/lint?profile=deep&external_egress=true").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(provider.calls.load(Ordering::SeqCst), 1);
    assert!(decoded
        .expect("typed deep report")
        .checks()
        .iter()
        .any(|check| {
            check.check_id() == "memories.semantic.classification"
                && check.outcome() == LintOutcome::Pass
        }));
}

#[tokio::test]
async fn active_lint_excludes_background_maintenance_until_the_report_finishes() {
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let provider: Arc<dyn LlmProvider> = Arc::new(BlockingApiProvider {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    });
    let fixture = Fixture::new_with_state(Vec::new(), None, move |state| {
        state.synthesis_llm = Some(provider);
    })
    .await;
    fixture.seed_semantic_candidates().await;

    let app = fixture.app.clone();
    let lint = tokio::spawn(async move {
        app.oneshot(request(
            Method::GET,
            "/api/lint?profile=deep&external_egress=true",
        ))
        .await
        .expect("lint response")
    });
    entered.notified().await;

    assert!(
        fixture
            .maintenance_coordinator
            .try_begin_background()
            .is_none(),
        "a live lint snapshot must exclude background writers"
    );
    release.notify_one();
    assert_eq!(lint.await.unwrap().status(), StatusCode::OK);
    drop(
        fixture
            .maintenance_coordinator
            .try_begin_background()
            .expect("background work resumes after lint"),
    );
}

#[tokio::test]
async fn lint_external_egress_is_rejected_for_general_profile_before_runner_work() {
    let fixture = Fixture::new(Vec::new(), None).await;
    let before_events = fixture.lint_events.events();

    let response = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/lint?external_egress=true"))
        .await
        .expect("invalid egress response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("invalid egress body");

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let error = serde_json::from_slice::<LintErrorResponse>(&body).expect("typed error");
    assert_eq!(
        error,
        LintErrorResponse::new("external_egress_requires_deep")
    );
    assert_eq!(fixture.lint_events.events(), before_events);
}

#[tokio::test]
async fn lint_deep_records_external_egress_consent_even_without_a_ready_provider() {
    let fixture = Fixture::new(Vec::new(), None).await;

    let (_, _, local) = report(&fixture, "/api/lint?profile=deep").await;
    let (_, _, external) = report(&fixture, "/api/lint?profile=deep&external_egress=true").await;

    assert_ne!(
        local.expect("local deep report").config_fingerprint(),
        external
            .expect("external-consented deep report")
            .config_fingerprint(),
        "effective config must record operator egress consent even when no provider is ready"
    );
}

#[tokio::test]
async fn lint_calling_agent_prepare_and_submit_share_the_canonical_endpoint() {
    let fixture = Fixture::new(Vec::new(), None).await;
    fixture.seed_semantic_candidates().await;

    let (_, _, daemon_deep) = report(&fixture, "/api/lint?profile=deep").await;
    let (status, _, prepared) = report(&fixture, "/api/lint?profile=deep&agent_assist=true").await;
    assert_eq!(status, StatusCode::OK);
    let prepared = prepared.expect("typed prepare report");
    assert_ne!(
        daemon_deep
            .expect("daemon deep report")
            .config_fingerprint(),
        prepared.config_fingerprint(),
        "effective config must record calling-agent consent"
    );
    let work = prepared.agent_work().expect("agent work");
    assert!(prepared.checks().iter().any(|check| {
        check.check_id() == "memories.semantic.classification"
            && check.evidence().iter().any(|evidence| {
                matches!(
                    evidence,
                    wenlan_types::lint::LintEvidenceRef::ReasonCode {
                        reason_code: LintReasonCode::SemanticAgentAdjudicationRequired
                    }
                )
            })
    }));
    let submission = agent_submission(work);
    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/lint?profile=deep&agent_assist=true",
            &submission,
        ))
        .await
        .expect("agent submission response");
    assert_eq!(response.status(), StatusCode::OK);
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("agent submission body");
    let final_report: LintReport = serde_json::from_slice(&body).expect("typed final report");
    assert!(final_report.checks().iter().any(|check| {
        check.check_id() == "memories.semantic.classification"
            && check.outcome() == LintOutcome::Finding
    }));
}

#[tokio::test]
async fn lint_agent_assist_is_deep_only_and_read_only() {
    let fixture = Fixture::new(Vec::new(), None).await;
    fixture.seed_semantic_candidates().await;
    let before = fixture.fingerprint().await;

    let invalid = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/lint?agent_assist=true"))
        .await
        .expect("invalid agent-assist response");
    assert_eq!(invalid.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let invalid_body = to_bytes(invalid.into_body(), usize::MAX)
        .await
        .expect("invalid agent-assist body");
    assert_eq!(
        serde_json::from_slice::<LintErrorResponse>(&invalid_body).unwrap(),
        LintErrorResponse::new("agent_assist_requires_deep")
    );

    let (_, _, prepared) = report(&fixture, "/api/lint?profile=deep&agent_assist=true").await;
    let prepared = prepared.unwrap();
    let submission = agent_submission(prepared.agent_work().expect("agent work"));
    let response = fixture
        .app
        .clone()
        .oneshot(json_request(
            Method::POST,
            "/api/lint?profile=deep&agent_assist=true",
            &submission,
        ))
        .await
        .expect("agent submission response");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(fixture.fingerprint().await, before);
}

fn agent_submission(work: &LintAgentWork) -> LintAgentSubmission {
    let verdicts = work
        .candidates()
        .iter()
        .map(|candidate| {
            let finding = candidate.check_id() == LintSemanticCheckId::MemoryClassification;
            LintAgentVerdict::try_new(
                candidate.reference(),
                if finding {
                    LintSemanticDecision::Finding
                } else {
                    LintSemanticDecision::Pass
                },
                None,
                candidate.reason_code(),
                9000,
                Vec::new(),
            )
            .unwrap()
        })
        .collect();
    LintAgentSubmission::try_new(work.work_digest().clone(), verdicts).unwrap()
}

#[tokio::test]
async fn lint_finding_response_stays_complete_and_typed() {
    let source = Source {
        display_name: None,
        id: String::new(),
        source_type: SourceType::Directory,
        path: PathBuf::new(),
        status: SyncStatus::Active,
        last_sync: None,
        file_count: 0,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: None,
        queued_files: 0,
        waiting_files: 0,
    };
    let fixture = Fixture::new(vec![source], None).await;

    let (status, _, decoded) = report(&fixture, "/api/lint").await;

    let decoded = decoded.expect("shared LintReport");
    assert_eq!(status, StatusCode::OK);
    assert!(decoded.complete());
    assert!(decoded.totals().findings() > 0);
    assert!(decoded.checks().iter().any(|check| {
        check.check_id() == "operations.source_configuration"
            && check.outcome() == LintOutcome::Finding
    }));
}

#[tokio::test]
async fn lint_check_failure_stays_inside_incomplete_report() {
    let missing = PathBuf::from("/definitely/missing/task-16-page-root");
    let fixture = Fixture::new(Vec::new(), Some(missing)).await;

    let (status, _, decoded) = report(&fixture, "/api/lint").await;

    let decoded = decoded.expect("shared LintReport");
    assert_eq!(status, StatusCode::OK);
    assert!(!decoded.complete());
    assert!(decoded.totals().incomplete() > 0);
}

#[tokio::test]
async fn lint_registered_scope_is_applied_by_core() {
    let fixture = Fixture::new(Vec::new(), None).await;
    fixture
        .db
        .create_space("work", None, false)
        .await
        .expect("space");

    let (status, _, decoded) = report(&fixture, "/api/lint?space=work").await;

    let decoded = decoded.expect("shared LintReport");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decoded.scope().kind(), LintScopeKind::Registered);
    assert!(decoded.scope().opaque_scope_ref().is_some());
}

#[tokio::test]
async fn lint_uncategorized_scope_is_applied_by_core() {
    let fixture = Fixture::new(Vec::new(), None).await;

    let (status, _, decoded) = report(&fixture, "/api/lint?space=uncategorized").await;

    let decoded = decoded.expect("shared LintReport");
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decoded.scope().kind(), LintScopeKind::Uncategorized);
    assert!(decoded.scope().opaque_scope_ref().is_none());
}

#[tokio::test]
async fn lint_unknown_scope_fails_closed_before_later_stages() {
    let missing = PathBuf::from("/definitely/missing/task-16-must-not-scan");
    let fixture = Fixture::new(Vec::new(), Some(missing)).await;

    let response = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/lint?space=missing"))
        .await
        .expect("invalid scope response");
    let status = response.status();
    let body = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("invalid scope body");

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let error = serde_json::from_slice::<LintErrorResponse>(&body).expect("typed error");
    assert_eq!(error, LintErrorResponse::new("invalid_scope"));
    assert_eq!(
        fixture.lint_events.events(),
        vec![
            LintRunEvent::ScopeValidation,
            LintRunEvent::TransactionQuery,
        ],
        "scope validation must own the first transaction query and stop all later stages"
    );
}

#[tokio::test]
async fn lint_route_rejects_unsupported_method_and_wiki_route_is_absent() {
    let fixture = Fixture::new(Vec::new(), None).await;

    let put = fixture
        .app
        .clone()
        .oneshot(request(Method::PUT, "/api/lint"))
        .await
        .expect("method response");
    let wiki = fixture
        .app
        .clone()
        .oneshot(request(Method::GET, "/api/wiki/check"))
        .await
        .expect("wiki response");

    assert_eq!(put.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(wiki.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn lint_endpoint_does_not_mutate_database_or_page_tree() {
    let fixture = Fixture::new(Vec::new(), None).await;
    let before = fixture.fingerprint().await;

    let (status, _, _) = report(&fixture, "/api/lint").await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(fixture.fingerprint().await, before);
}

#[tokio::test]
async fn lint_endpoint_uses_configured_process_clock_for_age_buckets() {
    let source = Source {
        display_name: None,
        id: "clock-source".to_string(),
        source_type: SourceType::Directory,
        path: PathBuf::from("clock-source"),
        status: SyncStatus::Active,
        last_sync: None,
        file_count: 1,
        memory_count: 0,
        last_sync_errors: 0,
        last_sync_error_detail: None,
        space: None,
        queued_files: 0,
        waiting_files: 0,
    };
    let fixture = Fixture::new_at(vec![source], None, Some(1_900_000_000)).await;
    fixture
        .db
        .upsert_sync_state("clock-source", "fixture.md", 1, "fixture-hash")
        .await
        .expect("sync state");

    let (status, _, report) = report(&fixture, "/api/lint").await;

    assert_eq!(status, StatusCode::OK);
    let source_check = report
        .expect("typed report")
        .checks()
        .iter()
        .find(|check| check.check_id() == "operations.source_configuration")
        .expect("source check")
        .clone();
    assert!(source_check.metrics().iter().any(|metric| {
        metric.code() == LintMetricCode::OperationAgeSevenDaysOrMore
            && metric.value() == &LintMetricValue::Count { value: 1 }
    }));
}
