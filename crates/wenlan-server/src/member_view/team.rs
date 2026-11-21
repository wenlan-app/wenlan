//! Typed response constructors for the team listener's member-facing routes.
//!
//! These constructors deliberately mirror the handful of wire shapes the
//! member API exposes. Keep this module narrow: a route should not be able to
//! turn an arbitrary JSON value, response, or caller supplied header into a
//! member response.

use axum::body::Body;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use super::MemberResponse;

#[derive(serde::Serialize)]
struct MeWithPeople {
    #[serde(flatten)]
    me: wenlan_types::team::TeamMe,
    people: Vec<wenlan_types::team::TeamPerson>,
}

pub(crate) fn me(
    me: wenlan_types::team::TeamMe,
    people: Vec<wenlan_types::team::TeamPerson>,
) -> MemberResponse {
    MemberResponse::new(Json(MeWithPeople { me, people }).into_response())
}

pub(crate) fn files(value: wenlan_types::team::TeamFileList) -> MemberResponse {
    MemberResponse::new(Json(value).into_response())
}

pub(crate) fn rules_proposals(value: Vec<wenlan_types::team::TeamRulesProposal>) -> MemberResponse {
    MemberResponse::new(Json(value).into_response())
}

pub(crate) fn rules_review_failure(
    error: wenlan_core::rulesync_team::ReviewError,
) -> MemberResponse {
    use wenlan_core::error::WenlanError;
    use wenlan_core::rulesync_team::ReviewError;
    let status = match &error {
        ReviewError::Forbidden => StatusCode::FORBIDDEN,
        ReviewError::NotConfigured | ReviewError::Failed(WenlanError::NotFound(_)) => {
            StatusCode::NOT_FOUND
        }
        ReviewError::Failed(WenlanError::Conflict(_)) => StatusCode::CONFLICT,
        ReviewError::Failed(WenlanError::Validation(_)) => StatusCode::UNPROCESSABLE_ENTITY,
        _ => return MemberResponse::status(StatusCode::SERVICE_UNAVAILABLE),
    };
    tracing::warn!(%error, "member Rules operation refused");
    MemberResponse::text(status, "The Rules operation could not be completed.")
}

pub(crate) fn version_validation_failure(error: wenlan_core::error::WenlanError) -> MemberResponse {
    tracing::warn!(%error, "member version rejected");
    MemberResponse::text(
        StatusCode::UNPROCESSABLE_ENTITY,
        "The version could not be stored.",
    )
}

pub(crate) fn space_file(value: wenlan_core::member_view::MemberSpaceFile) -> MemberResponse {
    MemberResponse::new(Json(value).into_response())
}

pub(crate) fn validated_markdown_file(
    file: wenlan_core::member_view::MemberVersionFile,
) -> MemberResponse {
    markdown_download(file.into_bytes())
}

pub(crate) fn markdown_download(bytes: Vec<u8>) -> MemberResponse {
    let mut response = (
        [(header::CONTENT_TYPE, "text/markdown; charset=utf-8")],
        Body::from(bytes),
    )
        .into_response();
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    MemberResponse::new(response)
}

/// A hosted MCP exchange. Only the member's MCP service can answer it: its
/// tools read the daemon through the member response types above.
pub(crate) async fn hosted_mcp(
    service: crate::team_routes::McpService,
    request: axum::extract::Request,
) -> MemberResponse {
    use tower::ServiceExt as _;
    match service.oneshot(request).await {
        Ok(response) => MemberResponse::new(response.into_response()),
        Err(never) => match never {},
    }
}

pub(crate) fn empty(status: StatusCode) -> MemberResponse {
    MemberResponse::status(status)
}

pub(crate) fn auth_text(status: StatusCode, text: &'static str) -> MemberResponse {
    let mut response = (status, text).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    MemberResponse::new(response)
}

pub(crate) fn rate_limited() -> MemberResponse {
    let mut response =
        auth_text(StatusCode::TOO_MANY_REQUESTS, "Too many requests.").into_response();
    response
        .headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from_static("5"));
    MemberResponse::new(response)
}

pub(crate) fn logged_out(clear_cookie: &str) -> MemberResponse {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(clear_cookie).expect("cookie attributes are a valid header"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    MemberResponse::new(response)
}

pub(crate) fn stored_version(created: bool) -> MemberResponse {
    MemberResponse::status(if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    })
}

pub(crate) fn providers(value: wenlan_types::team::TeamProviders) -> MemberResponse {
    MemberResponse::new(Json(value).into_response())
}

pub(crate) fn started_url(url: String, cookie: &str) -> MemberResponse {
    #[derive(serde::Serialize)]
    struct StartedUrl {
        url: String,
    }
    let mut response = Json(StartedUrl { url }).into_response();
    append_cookie(&mut response, cookie);
    MemberResponse::new(response)
}

#[derive(Clone, Copy)]
pub(crate) enum AuthErrorCode {
    ReauthRequired,
    NoLinkedAccount,
    InviteUnusable,
}

impl AuthErrorCode {
    fn as_str(self) -> &'static str {
        match self {
            Self::ReauthRequired => "reauth_required",
            Self::NoLinkedAccount => "no_linked_account",
            Self::InviteUnusable => "invite_unusable",
        }
    }
}

pub(crate) fn auth_error(status: StatusCode, code: AuthErrorCode) -> MemberResponse {
    #[derive(serde::Serialize)]
    struct ErrorBody {
        error: &'static str,
    }
    let mut response = Json(ErrorBody {
        error: code.as_str(),
    })
    .into_response();
    *response.status_mut() = status;
    MemberResponse::new(response)
}

pub(crate) enum InvitePage {
    WaitingForApproval,
    Refused(wenlan_core::team::InviteRefusal),
}

pub(crate) fn invite_page(
    status: StatusCode,
    page: InvitePage,
    clear_cookie: &str,
) -> MemberResponse {
    let message = match page {
        InvitePage::WaitingForApproval => "Thanks. The owner of this team needs to approve you before you can sign in. Ask them, then come back and sign in.".to_string(),
        InvitePage::Refused(reason) => match reason {
            wenlan_core::team::InviteRefusal::Unknown => "This invite does not work. Ask the owner of the team for a new link.".into(),
            wenlan_core::team::InviteRefusal::Expired => "This invite has expired. Ask the owner of the team for a new link.".into(),
            wenlan_core::team::InviteRefusal::Used => "This invite has already been used. Ask the owner of the team for a new link.".into(),
            wenlan_core::team::InviteRefusal::IdentityTaken => "This account already belongs to another member of this team.".into(),
            wenlan_core::team::InviteRefusal::EmailMismatch { invited } => format!(
                "This invite is for {invited}. Go back to the link and sign in with that account."
            ),
        },
    };
    let mut response = (status, message).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    append_cookie(&mut response, clear_cookie);
    MemberResponse::new(response)
}

pub(crate) enum CallbackFailure {
    Refused,
    NotInTeam,
    Unavailable,
}

pub(crate) fn callback_failure(failure: CallbackFailure, clear_cookie: &str) -> MemberResponse {
    let (status, message) = match failure {
        CallbackFailure::Refused => (
            StatusCode::BAD_REQUEST,
            "Sign-in did not work. Go back and try again.",
        ),
        CallbackFailure::NotInTeam => (StatusCode::FORBIDDEN, "This account is not in this team."),
        CallbackFailure::Unavailable => (
            StatusCode::BAD_GATEWAY,
            "Sign-in is not available right now. Try again later.",
        ),
    };
    let mut response = (status, message).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    append_cookie(&mut response, clear_cookie);
    MemberResponse::new(response)
}

pub(crate) fn redirect_to_provider(url: &str, cookie: &str) -> MemberResponse {
    let mut response = redirect_response(url);
    append_cookie(&mut response, cookie);
    MemberResponse::new(response)
}

pub(crate) fn redirect_to_app(session_cookie: &str, clear_cookie: &str) -> MemberResponse {
    let mut response = redirect_response("/app");
    append_cookie(&mut response, session_cookie);
    append_cookie(&mut response, clear_cookie);
    MemberResponse::new(response)
}

fn redirect_response(location: &str) -> Response {
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("validated redirect is a header value"),
    );
    response
}

fn append_cookie(response: &mut Response, cookie: &str) {
    response.headers_mut().append(
        header::SET_COOKIE,
        HeaderValue::from_str(cookie).expect("cookie values are visible ASCII"),
    );
}
