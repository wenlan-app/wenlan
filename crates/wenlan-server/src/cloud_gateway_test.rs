// SPDX-License-Identifier: AGPL-3.0-only
use super::*;
use crate::cloud_locator::Endpoint;
use crate::team_routes::auth::{GithubUrls, Provider};
use axum::http::Method;
use serde_json::{json, Value};
use tower::ServiceExt;

const ORIGIN: &str = "https://cloud.example.com";

/// Stands in for the locator: every person's Wenlan is the one fake, or
/// the machine is full.
struct FakeLocator {
    endpoint: Endpoint,
    busy: bool,
    asked: std::sync::Mutex<Vec<AccountId>>,
}

impl Locator for FakeLocator {
    async fn locate(&self, account: &AccountId) -> Result<Endpoint, LocateError> {
        self.asked.lock().unwrap().push(account.clone());
        if self.busy {
            return Err(LocateError::Busy);
        }
        Ok(self.endpoint)
    }
}

/// A person's Wenlan that answers with what it was sent and tries to set
/// a cookie of its own.
async fn serve_wenlan() -> Endpoint {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new().fallback(|req: Request| async move {
        let (parts, body) = req.into_parts();
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        let seen = json!({
            "method": parts.method.as_str(),
            "uri": parts.uri.to_string(),
            "pass": header(PASS_HEADER),
            "cookie": header("cookie"),
            "authorization": header("authorization"),
            "origin": header("origin"),
        });
        let body = axum::body::to_bytes(body, 1 << 20).await.unwrap();
        let mut response = axum::Json(json!({
            "seen": seen,
            "body": String::from_utf8_lossy(&body),
        }))
        .into_response();
        *response.status_mut() = StatusCode::CREATED;
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_static("own=1; Path=/"),
        );
        response
    });
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Endpoint { addr }
}

/// A fake GitHub that signs in user 42 with the given verified emails.
async fn serve_github(emails: Value) -> GithubUrls {
    use axum::routing::{get, post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = axum::Router::new()
        .route(
            "/token",
            post(
                |axum::Form(form): axum::Form<HashMap<String, String>>| async move {
                    if form.get("code").map(String::as_str) == Some("good") {
                        axum::Json(json!({"access_token": "t", "token_type": "bearer"}))
                    } else {
                        axum::Json(json!({"error": "bad_verification_code"}))
                    }
                },
            ),
        )
        .route("/api/user", get(|| async { axum::Json(json!({"id": 42})) }))
        .route(
            "/api/user/emails",
            get(move || {
                let emails = emails.clone();
                async move { axum::Json(emails) }
            }),
        );
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    GithubUrls {
        authorize: format!("{base}/authorize"),
        token: format!("{base}/token"),
        api: format!("{base}/api"),
    }
}

struct Env {
    dir: tempfile::TempDir,
    gateway: Arc<Gateway<FakeLocator>>,
    key: PassKey,
}

async fn env_with(busy: bool, emails: Value) -> Env {
    let dir = tempfile::tempdir().unwrap();
    let urls = serve_github(emails).await;
    let http = openidconnect::reqwest::ClientBuilder::new()
        .no_proxy()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let auth =
        AuthState::new(vec![Provider::github_with_urls("id", "secret", urls)]).with_http(http);
    let key = PassKey::random().unwrap();
    let gateway = Arc::new(Gateway::new(
        Arc::new(FakeLocator {
            endpoint: serve_wenlan().await,
            busy,
            asked: Default::default(),
        }),
        auth,
        GatewaySettings {
            origin: ORIGIN.into(),
            root: dir.path().to_path_buf(),
        },
        key.clone(),
    ));
    Env { dir, gateway, key }
}

async fn env() -> Env {
    env_with(
        false,
        json!([{"email": "alice@example.com", "verified": true, "primary": true}]),
    )
    .await
}

async fn send(env: &Env, req: axum::http::Request<Body>) -> Response {
    router(env.gateway.clone()).oneshot(req).await.unwrap()
}

fn get(uri: &str, headers: &[(&str, &str)]) -> axum::http::Request<Body> {
    let mut req = axum::http::Request::builder().method(Method::GET).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, *value);
    }
    req.body(Body::empty()).unwrap()
}

fn cookie_named(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|c| c.strip_prefix(&format!("{name}=")))
        .map(|rest| rest.split(';').next().unwrap_or("").to_string())
        .filter(|value| !value.is_empty())
}

async fn json_of(response: Response) -> Value {
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Signs in with the fake GitHub; the session cookie, or the response.
async fn sign_in(env: &Env) -> Result<String, Response> {
    let started = send(env, get("/auth/start/github", &[])).await;
    assert_eq!(started.status(), StatusCode::FOUND);
    let location = started.headers()[header::LOCATION].to_str().unwrap();
    let state = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "state")
        .unwrap()
        .1
        .to_string();
    let login = cookie_named(&started, "__Host-wenlan-login").unwrap();
    let back = send(
        env,
        get(
            &format!("/auth/callback/github?state={state}&code=good"),
            &[("cookie", &format!("__Host-wenlan-login={login}"))],
        ),
    )
    .await;
    if back.status() != StatusCode::FOUND {
        return Err(back);
    }
    assert_eq!(back.headers()[header::LOCATION], "/app/");
    Ok(cookie_named(&back, "__Host-wenlan-session").expect("a session"))
}

#[test]
fn an_account_is_attached_to_the_first_provider_account_that_owns_its_email() {
    let dir = tempfile::tempdir().unwrap();
    let alice = add_account(dir.path(), "Alice@Example.com").unwrap();
    assert!(add_account(dir.path(), "alice@example.com").is_err());
    assert!(add_account(dir.path(), "no-at-sign").is_err());
    let emails = |list: &[&str]| list.iter().map(|e| e.to_string()).collect::<Vec<_>>();
    let proof = |subject: &'static str, trusted: &'static [String]| Proof {
        issuer: "https://github.com",
        subject,
        trusted_emails: trusted,
    };

    let none: &'static [String] = Box::leak(emails(&[]).into_boxed_slice());
    let hers: &'static [String] = Box::leak(emails(&["alice@example.com"]).into_boxed_slice());
    // Nobody's email: no account.
    assert_eq!(account_for(dir.path(), &proof("1", none)).unwrap(), None);
    // Her email, vouched for: attached.
    assert_eq!(
        account_for(dir.path(), &proof("1", hers)).unwrap(),
        Some(alice.clone())
    );
    // From then on the provider account alone finds it.
    assert_eq!(
        account_for(dir.path(), &proof("1", none)).unwrap(),
        Some(alice.clone())
    );
    // A second GitHub account with the same email does not take it over.
    assert_eq!(account_for(dir.path(), &proof("2", hers)).unwrap(), None);
    let table = read_accounts(dir.path()).unwrap();
    assert_eq!(table.accounts.len(), 1);
    assert_eq!(table.accounts[0].identities.len(), 1);
}

#[tokio::test]
async fn a_person_signs_in_and_reaches_their_own_wenlan_with_a_pass() {
    let env = env().await;
    let alice = add_account(env.dir.path(), "alice@example.com").unwrap();
    let session = sign_in(&env).await.unwrap();

    let req = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/team/v1/app/pages?x=1")
        .header("cookie", format!("__Host-wenlan-session={session}"))
        .header("authorization", "Bearer someone-elses")
        .header(PASS_HEADER, "v1.forged")
        .header("origin", ORIGIN)
        .body(Body::from("hello"))
        .unwrap();
    let response = send(&env, req).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert!(
        response.headers().get(header::SET_COOKIE).is_none(),
        "the Wenlan's own cookies stay behind"
    );
    let answer = json_of(response).await;
    let seen = &answer["seen"];
    assert_eq!(seen["method"], "POST");
    assert_eq!(seen["uri"], "/team/v1/app/pages?x=1");
    assert_eq!(answer["body"], "hello");
    assert_eq!(seen["cookie"], Value::Null);
    assert_eq!(seen["authorization"], Value::Null);
    assert_eq!(seen["origin"], ORIGIN);
    let pass = seen["pass"].as_str().unwrap();
    assert_eq!(
        env.key.for_account(&alice).verify(pass),
        Some(alice.clone())
    );
    assert_eq!(*env.gateway.locator.asked.lock().unwrap(), vec![alice]);
}

#[tokio::test]
async fn someone_without_an_account_is_not_signed_in() {
    let env = env().await;
    add_account(env.dir.path(), "bob@example.com").unwrap();
    let refused = sign_in(&env).await.unwrap_err();
    assert_eq!(refused.status(), StatusCode::FORBIDDEN);
    assert_eq!(cookie_named(&refused, "__Host-wenlan-session"), None);
}

#[tokio::test]
async fn an_unverified_email_does_not_claim_an_account() {
    let env = env_with(
        false,
        json!([{"email": "alice@example.com", "verified": false, "primary": true}]),
    )
    .await;
    add_account(env.dir.path(), "alice@example.com").unwrap();
    assert_eq!(
        sign_in(&env).await.unwrap_err().status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn nothing_is_forwarded_before_sign_in() {
    let env = env().await;
    let page = send(&env, get("/app/", &[("accept", "text/html")])).await;
    assert_eq!(page.status(), StatusCode::UNAUTHORIZED);
    let html = String::from_utf8(
        axum::body::to_bytes(page.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    assert!(html.contains("/auth/start/github"));
    let api = send(
        &env,
        get(
            "/team/v1/app/pages",
            &[("cookie", "__Host-wenlan-session=made-up")],
        ),
    )
    .await;
    assert_eq!(api.status(), StatusCode::UNAUTHORIZED);
    assert!(env.gateway.locator.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_persons_wenlans_sign_in_routes_are_never_reached() {
    let env = env().await;
    add_account(env.dir.path(), "alice@example.com").unwrap();
    let session = sign_in(&env).await.unwrap();
    let cookie = format!("__Host-wenlan-session={session}");
    let response = send(&env, get("/auth/device/token", &[("cookie", &cookie)])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert!(env.gateway.locator.asked.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_full_machine_says_so() {
    let env = env_with(
        true,
        json!([{"email": "alice@example.com", "verified": true, "primary": true}]),
    )
    .await;
    add_account(env.dir.path(), "alice@example.com").unwrap();
    let session = sign_in(&env).await.unwrap();
    let response = send(
        &env,
        get(
            "/team/v1/app/pages",
            &[("cookie", &format!("__Host-wenlan-session={session}"))],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn signing_out_ends_the_session() {
    let env = env().await;
    add_account(env.dir.path(), "alice@example.com").unwrap();
    let session = sign_in(&env).await.unwrap();
    let cookie = format!("__Host-wenlan-session={session}");
    let out = axum::http::Request::builder()
        .method(Method::POST)
        .uri("/auth/signout")
        .header("cookie", &cookie)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&env, out).await.status(), StatusCode::NO_CONTENT);
    let after = send(&env, get("/team/v1/app/pages", &[("cookie", &cookie)])).await;
    assert_eq!(after.status(), StatusCode::UNAUTHORIZED);
}
