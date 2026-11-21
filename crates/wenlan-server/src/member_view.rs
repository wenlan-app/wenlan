// SPDX-License-Identifier: AGPL-3.0-only
//! The response boundary at the server's member route registrations.

use axum::extract::{FromRequest, FromRequestParts, Request};
use axum::handler::Handler;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::MethodRouter;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use crate::error::ServerError;
use wenlan_core::member_view as core_view;
use wenlan_types::responses as wire;

pub(crate) mod team;

/// Only this module can turn a payload into a member response.
pub(crate) struct MemberResponse(Response);
#[derive(Clone)]
struct Typed;

impl MemberResponse {
    fn new(response: Response) -> Self {
        Self(response)
    }

    pub(crate) fn status(status: StatusCode) -> Self {
        Self::new(status.into_response())
    }

    pub(crate) fn text(status: StatusCode, text: &'static str) -> Self {
        Self::new((status, text).into_response())
    }

    pub(crate) fn json<T: MemberBody>(status: StatusCode, body: T) -> Self {
        Self::new((status, Json(body)).into_response())
    }

    pub(crate) fn problem(status: StatusCode, error: &'static str) -> Self {
        Self::json(status, MemberProblem::new(error))
    }

    pub(crate) fn default_problem(status: StatusCode) -> Self {
        if status.is_server_error() {
            return Self::status(status);
        }
        let message = match status {
            StatusCode::NOT_FOUND => "page not found",
            StatusCode::FORBIDDEN => "This route is unavailable to team members.",
            StatusCode::CONFLICT => "The page changed. Read it again and retry.",
            _ => "This request could not be completed.",
        };
        Self::problem(status, message)
    }

    pub(crate) fn from_error(error: ServerError) -> Self {
        let status = match &error {
            ServerError::MemberProblem { status, body } => {
                return Self::json(*status, body.clone());
            }
            ServerError::Structured { status, body } => {
                let mut body = body.clone();
                if let Some(fields) = body.as_object_mut() {
                    fields.remove("existing_page_id");
                    fields.remove("existing_page_title");
                }
                return Self::project_problem(*status, body);
            }
            ServerError::NotFound(_) => StatusCode::NOT_FOUND,
            ServerError::Conflict(message)
                if message == wenlan_core::citations::MEMBER_CITATION_CONFLICT =>
            {
                return Self::problem(
                    StatusCode::CONFLICT,
                    wenlan_core::citations::MEMBER_CITATION_CONFLICT,
                );
            }
            ServerError::Conflict(_) => StatusCode::CONFLICT,
            ServerError::ValidationError(_) | ServerError::QualityGateRejected { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            ServerError::DbNotInitialized => StatusCode::SERVICE_UNAVAILABLE,
            ServerError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            ServerError::Upstream(_) => StatusCode::BAD_GATEWAY,
            ServerError::AgentDisabled(_) => StatusCode::FORBIDDEN,
            _ => StatusCode::BAD_REQUEST,
        };
        Self::default_problem(status)
    }

    /// The browser accepts only known problem codes, even from its daemon.
    pub(crate) fn project_problem(status: StatusCode, body: serde_json::Value) -> Self {
        if status.is_server_error() {
            return Self::status(status);
        }
        let Ok(body) = serde_json::from_value::<MemberProblem>(body) else {
            return Self::default_problem(status);
        };
        let Some(code) = body.code else {
            return Self::default_problem(status);
        };
        let problem = MemberProblem {
            error: code.message().into(),
            code: Some(code),
            current_version: code
                .reports_version()
                .then_some(body.current_version)
                .flatten(),
            existing_page_id: (code == ProblemCode::PageTitleConflict)
                .then_some(body.existing_page_id)
                .flatten(),
            existing_page_title: (code == ProblemCode::PageTitleConflict)
                .then_some(body.existing_page_title)
                .flatten(),
        };
        Self::json(status, problem)
    }
}

impl IntoResponse for MemberResponse {
    fn into_response(mut self) -> Response {
        self.0.extensions_mut().insert(Typed);
        self.0
    }
}

#[cfg(test)]
pub(crate) fn is_typed(response: &Response) -> bool {
    response.extensions().get::<Typed>().is_some()
}

pub(crate) fn verified_forwarded_response(response: Response) -> Response {
    if response.extensions().get::<Typed>().is_some() {
        return response;
    }
    let status = response.status();
    if status.is_success() || status.is_redirection() {
        tracing::error!(%status, "A forwarded request reached an untyped response");
        return MemberResponse::status(StatusCode::INTERNAL_SERVER_ERROR).into_response();
    }
    MemberResponse::default_problem(status).into_response()
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProblemCode {
    PageDraftNotFound,
    PageDraftIdConflict,
    DraftVersionConflict,
    PageTitleConflict,
    PageVersionConflict,
    PageTitleTaken,
    PageNotArchived,
    PageTitleFixed,
    PageNotRestorable,
    PageNotRemovable,
    PageIsThePersons,
    PermanentDeleteNeedsPerson,
}

impl ProblemCode {
    fn message(self) -> &'static str {
        match self {
            Self::PageDraftNotFound => "Page draft not found",
            Self::PageDraftIdConflict => "Page draft id already belongs to another Page",
            Self::DraftVersionConflict => "Page draft changed since it was loaded",
            Self::PageTitleConflict | Self::PageTitleTaken => {
                "A Page with this title already exists"
            }
            Self::PageVersionConflict => "The page changed. Read it again and retry.",
            Self::PageNotArchived => "This page is no longer archived.",
            Self::PageTitleFixed => "This page's title cannot be changed.",
            Self::PageNotRestorable => "This page cannot be restored.",
            Self::PageNotRemovable => "This page cannot be removed.",
            // The removal refusals name no page or person; the hosted MCP
            // client switches on these codes.
            Self::PageIsThePersons => wenlan_core::post_write::page_removal_refusal_message(
                wenlan_types::responses::PageRemovalErrorCode::PageIsThePersons,
            ),
            Self::PermanentDeleteNeedsPerson => {
                wenlan_core::post_write::page_removal_refusal_message(
                    wenlan_types::responses::PageRemovalErrorCode::PermanentDeleteNeedsPerson,
                )
            }
        }
    }

    fn reports_version(self) -> bool {
        matches!(self, Self::DraftVersionConflict | Self::PageVersionConflict)
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MemberProblem {
    error: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    code: Option<ProblemCode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    current_version: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    existing_page_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    existing_page_title: Option<String>,
}

impl std::fmt::Debug for MemberProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

impl MemberProblem {
    fn new(error: &'static str) -> Self {
        Self {
            error: error.into(),
            code: None,
            current_version: None,
            existing_page_id: None,
            existing_page_title: None,
        }
    }

    pub(crate) fn title_conflict(identity: Option<(String, String)>) -> Self {
        let (existing_page_id, existing_page_title) = identity.unzip();
        Self {
            code: Some(ProblemCode::PageTitleConflict),
            existing_page_id,
            existing_page_title,
            ..Self::new(ProblemCode::PageTitleConflict.message())
        }
    }

    pub(crate) fn message(&self) -> &str {
        &self.error
    }
}

/// A dual-use daemon handler can return a local or member view. The
/// forwarded route adapter below will only serialize the member variant.
pub enum PageResponse<Local, Member> {
    Local(Local),
    Member(Member),
}

impl<Local, Member> std::fmt::Debug for PageResponse<Local, Member> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local(_) => "Local response",
            Self::Member(_) => "Member response",
        })
    }
}

impl<Local: Serialize, Member: MemberBody> IntoResponse for PageResponse<Local, Member> {
    fn into_response(self) -> Response {
        match self {
            Self::Local(body) => Json(body).into_response(),
            Self::Member(body) => MemberResponse::json(StatusCode::OK, body).into_response(),
        }
    }
}

impl<Local, Member: From<Local>> PageResponse<Local, Member> {
    pub(crate) fn for_caller(local: Local, private: bool) -> Self {
        if private {
            Self::Local(local)
        } else {
            Self::Member(local.into())
        }
    }
}

#[cfg(test)]
impl<Local: serde::de::DeserializeOwned, Member: Serialize> PageResponse<Local, Member> {
    pub(crate) fn into_test_json(self) -> Json<Local> {
        Json(match self {
            Self::Local(body) => body,
            Self::Member(body) => {
                serde_json::from_value(serde_json::to_value(body).unwrap()).unwrap()
            }
        })
    }
}

mod seal {
    pub trait MemberBody {}
    pub trait DirectHandler<T> {}
}

/// Closed list of payloads the HTTP member boundary can serialize.
pub trait MemberBody: Serialize + seal::MemberBody {}

macro_rules! member_bodies {
    ($($body:ty),+ $(,)?) => {
        $(impl seal::MemberBody for $body {}
          impl MemberBody for $body {})+
    };
}

member_bodies!(
    core_view::MemberHealth,
    Vec<core_view::MemberSpace>,
    Vec<core_view::MemberChange>,
    Vec<core_view::MemberActivity>,
    core_view::MemberGraph,
    core_view::MemberPages<core_view::MemberPage>,
    core_view::MemberPages<core_view::MemberPageEntry>,
    core_view::MemberPageDetail,
    core_view::MemberPageRead,
    core_view::MemberRevisions,
    core_view::MemberManualWrite,
    core_view::MemberCreate,
    core_view::MemberWrite,
    core_view::MemberRemoval,
    core_view::MemberRestore,
    core_view::MemberDraft,
    core_view::MemberMove,
    core_view::MemberRules,
    core_view::MemberBasis,
    wire::MemberRenamePageResponse,
    MemberSources,
    MemberFolders,
    MemberFolder,
    MemberBasisRemoval,
    MemberRuleReport,
    MemberBrief,
    MemberRecall,
    MemberProblem,
);

trait ForwardedReply: IntoResponse {
    fn into_member(self) -> MemberResponse;
}

impl<L: Serialize, M: MemberBody> ForwardedReply for Result<PageResponse<L, M>, ServerError> {
    fn into_member(self) -> MemberResponse {
        match self {
            Ok(PageResponse::Member(body)) => MemberResponse::json(StatusCode::OK, body),
            Ok(PageResponse::Local(_)) => {
                tracing::error!("A forwarded handler selected its local response");
                MemberResponse::status(StatusCode::INTERNAL_SERVER_ERROR)
            }
            Err(error) => MemberResponse::from_error(error),
        }
    }
}

/// Axum erases a handler's return type. Check it before registration and
/// choose the wire response using the authenticated request, not the branch
/// chosen by the handler.
#[derive(Clone)]
pub(crate) struct Forwarded<H>(pub H);

macro_rules! typed_handler {
    ([$($part:ident),*], $last:ident) => {
        impl<H, Fut, M, $($part,)* $last> seal::DirectHandler<(M, $($part,)* $last,)> for H
        where
            H: FnOnce($($part,)* $last) -> Fut,
            Fut: Future<Output = MemberResponse>,
        {}

        #[allow(non_snake_case, unused_mut)]
        impl<H, Fut, R, S, M, $($part,)* $last> Handler<(M, $($part,)* $last,), S> for Forwarded<H>
        where
            H: FnOnce($($part,)* $last) -> Fut + Clone + Send + Sync + 'static,
            Fut: Future<Output = R> + Send,
            R: ForwardedReply,
            S: Send + Sync + 'static,
            $($part: FromRequestParts<S> + Send,)*
            $last: FromRequest<S, M> + Send,
        {
            type Future = Pin<Box<dyn Future<Output = Response> + Send>>;

            fn call(self, request: Request, state: S) -> Self::Future {
                let member = !crate::team_actor::can_read_private_memories(request.headers());
                let (mut parts, body) = request.into_parts();
                Box::pin(async move {
                    let rejection = |rejection: Response| {
                        if member {
                            MemberResponse::default_problem(rejection.status()).into_response()
                        } else {
                            rejection
                        }
                    };
                    $(let $part = match $part::from_request_parts(&mut parts, &state).await {
                        Ok(value) => value,
                        Err(error) => return rejection(error.into_response()),
                    };)*
                    let request = Request::from_parts(parts, body);
                    let $last = match $last::from_request(request, &state).await {
                        Ok(value) => value,
                        Err(error) => return rejection(error.into_response()),
                    };
                    let result = self.0($($part,)* $last).await;
                    if member {
                        result.into_member().into_response()
                    } else {
                        result.into_response()
                    }
                })
            }
        }
    };
}

typed_handler!([], A);
typed_handler!([A], B);
typed_handler!([A, B], C);
typed_handler!([A, B, C], D);
typed_handler!([A, B, C, D], E);
typed_handler!([A, B, C, D, E], F);
typed_handler!([A, B, C, D, E, F], G);
typed_handler!([A, B, C, D, E, F, G], I);

pub(crate) struct MemberMethod<S> {
    route: MethodRouter<S>,
    #[cfg(test)]
    pub(crate) method: &'static str,
}

impl<S: Clone + Send + Sync + 'static> MemberMethod<S> {
    pub(crate) fn install(self, router: axum::Router<S>, path: &str) -> axum::Router<S> {
        router.route(path, self.route)
    }

    pub(crate) fn body_limit(mut self, limit: usize) -> Self {
        self.route = self
            .route
            .layer(axum::extract::DefaultBodyLimit::max(limit));
        self
    }
}

macro_rules! direct_member_route {
    ($method:ident, $name:literal) => {
        pub(crate) fn $method<H, T, S>(handler: H) -> MemberMethod<S>
        where
            H: Handler<T, S> + seal::DirectHandler<T>,
            T: 'static,
            S: Clone + Send + Sync + 'static,
        {
            MemberMethod {
                route: axum::routing::$method(handler),
                #[cfg(test)]
                method: $name,
            }
        }
    };
}

direct_member_route!(get, "GET");
direct_member_route!(post, "POST");
direct_member_route!(put, "PUT");
direct_member_route!(any, "ANY");

pub type MemberSources = [(); 0];

#[derive(Serialize, Deserialize)]
pub struct MemberFolders {
    pub(crate) folders: Vec<MemberFolderEntry>,
    pub(crate) truncated: bool,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct MemberFolderEntry {
    pub path: String,
    pub parent_path: String,
    pub name: String,
}

#[derive(Serialize, Deserialize)]
pub struct MemberFolder {
    pub(crate) path: String,
}

#[derive(Serialize, Deserialize)]
pub struct MemberBasisRemoval {
    pub(crate) removed: bool,
}

impl From<wenlan_types::BasisRemoveResponse> for MemberBasisRemoval {
    fn from(response: wenlan_types::BasisRemoveResponse) -> Self {
        Self {
            removed: response.removed,
        }
    }
}

#[derive(Serialize)]
pub struct MemberRuleReport {
    pub(crate) recorded: bool,
    pub(crate) rule_id: Option<String>,
}

impl From<wenlan_types::skills::SkillReportResponse> for MemberRuleReport {
    fn from(response: wenlan_types::skills::SkillReportResponse) -> Self {
        Self {
            recorded: response.recorded,
            rule_id: response.rule_id,
        }
    }
}

#[derive(Serialize)]
pub struct MemberRecall {
    pub(crate) results: [(); 0],
    pub(crate) took_ms: f64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) supplemental_pages: Vec<wenlan_types::member_view::MemberSearchHit>,
}

#[derive(Serialize)]
pub struct MemberBrief {
    pub(crate) state: wenlan_types::BriefReadState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) space: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) related_context: Option<MemberBriefContext>,
}

#[derive(Serialize)]
pub(crate) struct MemberBriefContext {
    pub(crate) query: String,
    pub(crate) results: Vec<wenlan_types::member_view::MemberSearchHit>,
}
