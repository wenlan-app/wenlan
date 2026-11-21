// SPDX-License-Identifier: AGPL-3.0-only
//! Sign-in against a fake OpenID Connect provider: a local server with
//! discovery, signing keys and a token endpoint, whose id tokens are signed
//! with a throwaway RSA key (a test fixture, used nowhere else). GitHub is
//! tested against a second local fake with its token endpoint and API.

use super::super::team_router;
use super::*;
use axum::body::{to_bytes, Body};
use axum::http::{Method, Request};
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use base64::Engine;
use serde_json::{json, Value};
use tower::ServiceExt;

const ORIGIN: &str = "https://team.example.com";
const CLIENT_ID: &str = "client-id";
const KEY_DER: &str = "MIIEowIBAAKCAQEAsxOzjZFOavDytNVeYp6kunwLJXaqBVTLG/A6jYBUFblLV7VYn1Z3KwvKgkBvHuKn4dQLAedf/Ew26QpKaiij6ip3QMYPVG5enge/yjaY9HnIwCX/0AFurSQTsPFUELbmPw8Ylc6ZRSjHjISMpsBkMvbqFn0FPuAKXrsYD7tWYIs2c/RLiFdpPeCPdxtlGF8EueqfPNfs63dEEqIBJT6wIXNlhXJfJM8C13nTSOIat8suMsrSj/t9kDR3zIDvgRh6HS2IDrugD4eyxhZvuq2pzXu4b2uIQLiy0X9HCpUbOWw7rPOB3KOs5Fx9/wjVS+Ph0byAAnOu1qDB4mraJn3c3QIDAQABAoIBABG9CRtkEhg6Q1Rh0U5kCA235VPB8CAEkPT0xBwJouqe9uVaf4uTAU1ohtgFoWw2B+qw+iEnZeTkOMk3PX1MBsaWi8cFB3Y105YETv1tEJQg7x+HdBJK3gd1DCVKFmlscJy89lK6iwikfDXBWHxZEOPSfQgavLcIcsaE8mJ2jF0WKiiSOhmrHdHzmhE6V/SVfSXS/5Y4TDHqvBa6r3nwe/CdKWJwjhC7Gf9FNceYvyerMAudZXMFiXIh4IUeWY+BT2XxwExq9RMPSgUEDy15Wn176lNkEZQ59W9Z76j6/HEg63tFmzo25xWoTiJEO8wtDVI79Abn+iz/25PRlYcd0bECgYEA+2h43bzSCJQ42e7MyTp1v6Xi7GcWSfqNlUv6Eu3kRtwRBhEK5VdMixlUktdbaVEB/S4cVBMwNnzRHjtfSMRaXVDiEjRzJq8fm/IfpdeVRRHq4cMUagyRlWCONb5kEKko5CHbybMKLQNM81iZeZcVwzE89EUn0jvSLkQVfHjvT/ECgYEAtlkGa2I92nlFmExhNvxoAPPOYkBh5v8w4byjfGwtbk4Q1PdM097BdyqIi9ceYNwKjkyGJl+7Mw5T5TSLQplBq5xJeqMnOJ1E/8GsTwHh9xdLZtCGmTfoTxZJVO5uDLzefkpJxZjZeveXlGhKix7DHhLMc++xlIntdfduAkApR60CgYAjJIporIFLk7SSui/EHTjibkTXON3Dv5NVAEbXqItVYGO8zlsQjfB0LTlGQXZUtW8qvexV0EzYAQCWUkRSfqgHDUxcsRo7mRU5DlJgM095VTTZFBThchpToCig2JlSPuMpaJY5jp5ohmKWTZc+iOEQ5OwZVSVo3NXXg8rhGGKTcQKBgGW03hnDIBDdl23NHGQZAMGXO0Qd+P1o1RDzsJRTmjtuG4FNtnJuSqZ2+NZW1EDeOp0IHcMFnlpp4W10k0BATxqy/2EB43x2a+fjVIJ04F57hbkBEjIfttl16s0OeB9usQLr2J34FUW7F7bVPM7AB8CcZ6E6zme1Ttd/fMKQmvW5AoGBAN7B2652NjQN/Xv0TOQNUbSqQLkQX7Aw7nf5rRiB4SzBLreD67gmuNUe+N5i1xPuZPHUPHR9tr7A2aLAT2LWLMjecr/d+h5P3y7mWigBFP3yKctafgUoSjkOjxDAOhRzALAJX2jfO+d4FEyFC6zCI/Ab2RbC+LI7u8VXuEK84WGI";
const KEY_N: &str = "sxOzjZFOavDytNVeYp6kunwLJXaqBVTLG_A6jYBUFblLV7VYn1Z3KwvKgkBvHuKn4dQLAedf_Ew26QpKaiij6ip3QMYPVG5enge_yjaY9HnIwCX_0AFurSQTsPFUELbmPw8Ylc6ZRSjHjISMpsBkMvbqFn0FPuAKXrsYD7tWYIs2c_RLiFdpPeCPdxtlGF8EueqfPNfs63dEEqIBJT6wIXNlhXJfJM8C13nTSOIat8suMsrSj_t9kDR3zIDvgRh6HS2IDrugD4eyxhZvuq2pzXu4b2uIQLiy0X9HCpUbOWw7rPOB3KOs5Fx9_wjVS-Ph0byAAnOu1qDB4mraJn3c3Q";

struct Fake {
    claims: Mutex<Value>,
    verifier: Mutex<Option<String>>,
    key: ring::rsa::KeyPair,
    issuer: String,
}

impl Fake {
    fn set(&self, claims: Value) {
        *self.claims.lock().unwrap() = claims;
    }

    fn jwt(&self, claims: &Value) -> String {
        let header = json!({"alg": "RS256", "typ": "JWT", "kid": "k1"});
        let part = |v: &Value| URL_SAFE_NO_PAD.encode(v.to_string());
        let message = format!("{}.{}", part(&header), part(claims));
        let mut sig = vec![0u8; self.key.public().modulus_len()];
        self.key
            .sign(
                &ring::signature::RSA_PKCS1_SHA256,
                &ring::rand::SystemRandom::new(),
                message.as_bytes(),
                &mut sig,
            )
            .unwrap();
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(sig))
    }
}

async fn serve_fake() -> Arc<Fake> {
    use axum::routing::{get, post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let der = STANDARD.decode(KEY_DER).unwrap();
    let fake = Arc::new(Fake {
        claims: Mutex::new(json!({})),
        verifier: Mutex::new(None),
        key: ring::rsa::KeyPair::from_der(&der).unwrap(),
        issuer: issuer.clone(),
    });
    let discovery = json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
        "response_types_supported": ["code"],
        "subject_types_supported": ["public"],
        "id_token_signing_alg_values_supported": ["RS256"],
    });
    let jwks = json!({"keys": [{
        "kty": "RSA", "use": "sig", "alg": "RS256", "kid": "k1", "n": KEY_N, "e": "AQAB"
    }]});
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            get(move || {
                let d = discovery.clone();
                async move { Json(d) }
            }),
        )
        .route(
            "/jwks",
            get(move || {
                let j = jwks.clone();
                async move { Json(j) }
            }),
        )
        .route(
            "/token",
            post(
                |State(fake): State<Arc<Fake>>,
                 axum::Form(form): axum::Form<HashMap<String, String>>| async move {
                    *fake.verifier.lock().unwrap() = form.get("code_verifier").cloned();
                    let claims = fake.claims.lock().unwrap().clone();
                    Json(json!({
                        "access_token": "at",
                        "token_type": "Bearer",
                        "id_token": fake.jwt(&claims),
                    }))
                },
            ),
        )
        .with_state(fake.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    fake
}

const GH_ACCESS_TOKEN: &str = "gho_fake_access_token_value";
const GH_CODE: &str = "the-code";

/// An API call the fake saw: path, `Authorization`, `User-Agent`.
type ApiCall = (String, Option<String>, Option<String>);

/// A fake GitHub: the token endpoint checks the code, the client secret and
/// the PKCE verifier against the challenge the test says the login sent.
struct FakeGh {
    base: String,
    user: Mutex<Value>,
    emails: Mutex<Value>,
    challenge: Mutex<Option<String>>,
    token_forms: Mutex<Vec<HashMap<String, String>>>,
    api_calls: Mutex<Vec<ApiCall>>,
}

impl FakeGh {
    fn set(&self, user: Value, emails: Value) {
        *self.user.lock().unwrap() = user;
        *self.emails.lock().unwrap() = emails;
    }

    fn urls(&self) -> GithubUrls {
        GithubUrls {
            authorize: format!("{}/authorize", self.base),
            token: format!("{}/token", self.base),
            api: format!("{}/api", self.base),
        }
    }
}

async fn serve_github() -> Arc<FakeGh> {
    use axum::routing::{get, post};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let fake = Arc::new(FakeGh {
        base,
        user: Mutex::new(json!({})),
        emails: Mutex::new(json!([])),
        challenge: Mutex::new(None),
        token_forms: Mutex::new(Vec::new()),
        api_calls: Mutex::new(Vec::new()),
    });
    fn recorded(fake: &FakeGh, path: &str, headers: &HeaderMap) {
        let get = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
        };
        fake.api_calls.lock().unwrap().push((
            path.to_string(),
            get("authorization"),
            get("user-agent"),
        ));
    }
    let app = axum::Router::new()
        .route(
            "/token",
            post(
                |State(fake): State<Arc<FakeGh>>,
                 axum::Form(form): axum::Form<HashMap<String, String>>| async move {
                    fake.token_forms.lock().unwrap().push(form.clone());
                    let challenge = fake.challenge.lock().unwrap().clone();
                    let pkce_ok = match (form.get("code_verifier"), challenge) {
                        (Some(verifier), Some(challenge)) => {
                            URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())) == challenge
                        }
                        _ => false,
                    };
                    let ok = form.get("code").map(String::as_str) == Some(GH_CODE)
                        && form.get("client_secret").map(String::as_str) == Some("gh-secret")
                        && pkce_ok;
                    // GitHub answers a refusal with 200 and an `error`.
                    if ok {
                        Json(json!({
                            "access_token": GH_ACCESS_TOKEN,
                            "token_type": "bearer",
                            "scope": "read:user,user:email",
                        }))
                    } else {
                        Json(json!({"error": "bad_verification_code"}))
                    }
                },
            ),
        )
        .route(
            "/api/user",
            get(
                |State(fake): State<Arc<FakeGh>>, headers: HeaderMap| async move {
                    recorded(&fake, "/user", &headers);
                    Json(fake.user.lock().unwrap().clone())
                },
            ),
        )
        .route(
            "/api/user/emails",
            get(
                |State(fake): State<Arc<FakeGh>>, headers: HeaderMap| async move {
                    recorded(&fake, "/user/emails", &headers);
                    Json(fake.emails.lock().unwrap().clone())
                },
            ),
        )
        .with_state(fake.clone());
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    fake
}

struct Env {
    _dir: tempfile::TempDir,
    state: TeamState,
    fake: Arc<Fake>,
    gh: Arc<FakeGh>,
    amy: (Member, String),
    bo: (Member, String),
}

async fn env() -> Env {
    env_with(TRANSACTION_TTL, BURST, REFILL_PER_SEC, Some(ORIGIN)).await
}

async fn env_with(ttl: Duration, burst: f64, refill: f64, url: Option<&str>) -> Env {
    let fake = serve_fake().await;
    let gh = serve_github().await;
    let dir = tempfile::tempdir().unwrap();
    let team_path = dir.path().join(team::TEAM_FILE);
    let amy = team::add_member(&team_path, "Amy", &[]).unwrap();
    let bo = team::add_member(&team_path, "Bo", &[]).unwrap();
    team::set_url(&team_path, url).unwrap();
    let http = openidconnect::reqwest::ClientBuilder::new()
        .no_proxy()
        .redirect(openidconnect::reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let auth = AuthState::new(vec![
        Provider::new("google", &fake.issuer, CLIENT_ID, "client-secret"),
        Provider::github_with_urls("gh-client", "gh-secret", gh.urls()),
    ])
    .tuned(ttl, burst, refill)
    .with_http(http);
    Env {
        state: TeamState {
            settings_path: dir.path().join("settings.json"),
            team_path,
            db: None,
            daemon_url: "http://127.0.0.1:9".into(),
            in_process_daemon: None,
            mcp: Arc::default(),
            sessions: Arc::default(),
            auth: Arc::new(auth),
        },
        _dir: dir,
        fake,
        gh,
        amy,
        bo,
    }
}

impl Env {
    fn link_directly(&self, member: &Member, sub: &str, email: &str) {
        team::link_identity(
            &self.state.team_path,
            &member.id,
            Identity {
                issuer: self.fake.issuer.clone(),
                subject: sub.into(),
                email: Some(email.into()),
                linked_at: String::new(),
            },
        )
        .unwrap();
    }

    fn member(&self, member: &Member) -> Member {
        team::member_by_id(&self.state.team_path, &member.id)
            .unwrap()
            .unwrap()
    }

    /// Claims the provider would sign for a login begun as `started`.
    fn claims(&self, started: &Started, sub: &str, email: &str) -> Value {
        let now = chrono::Utc::now().timestamp();
        json!({
            "iss": self.fake.issuer,
            "aud": CLIENT_ID,
            "sub": sub,
            "email": email,
            "email_verified": true,
            "nonce": started.nonce,
            "exp": now + 300,
            "iat": now,
            "auth_time": now,
        })
    }
}

fn request(method: Method, uri: &str, headers: &[(&str, String)]) -> Request<Body> {
    let mut req = Request::builder().method(method).uri(uri);
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let mut req = req.body(Body::empty()).unwrap();
    req.extensions_mut().insert(ConnectInfo::<SocketAddr>(
        "203.0.113.7:4000".parse().unwrap(),
    ));
    req
}

async fn send(env: &Env, req: Request<Body>) -> Response {
    team_router(env.state.clone()).oneshot(req).await.unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

/// The value of a cookie the response sets (not a clearing one).
fn set_cookie_value(response: &Response, name: &str) -> Option<String> {
    response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|v| {
            let first = v.split(';').next()?;
            let (n, value) = first.split_once('=')?;
            (n == name && !value.is_empty()).then(|| value.to_string())
        })
}

fn login_cookie_of(response: &Response) -> String {
    set_cookie_value(response, LOGIN_COOKIE).expect("a login cookie is set")
}

struct Started {
    id: String,
    state: String,
    nonce: String,
    challenge: String,
    url: String,
}

fn parse_started(url: &str, id: String) -> Started {
    let parsed = url::Url::parse(url).unwrap();
    let get = |k: &str| {
        parsed
            .query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    };
    assert_eq!(get("code_challenge_method"), "S256");
    assert_eq!(get("response_type"), "code");
    assert_eq!(get("client_id"), CLIENT_ID);
    assert_eq!(
        get("redirect_uri"),
        format!("{ORIGIN}/auth/callback/google")
    );
    Started {
        id,
        state: get("state"),
        nonce: get("nonce"),
        challenge: get("code_challenge"),
        url: url.to_string(),
    }
}

async fn start_sign_in(env: &Env) -> Started {
    let response = send(
        env,
        request(Method::GET, "/auth/start/google?purpose=signin", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let id = login_cookie_of(&response);
    let url = response.headers()[header::LOCATION].to_str().unwrap();
    parse_started(url, id)
}

fn origin_header() -> (&'static str, String) {
    ("origin", ORIGIN.to_string())
}

/// Starts a Link with a legacy key.
async fn start_link(env: &Env, key: &str) -> Started {
    let response = send(
        env,
        request(
            Method::POST,
            "/auth/link/google",
            &[origin_header(), ("authorization", format!("Bearer {key}"))],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = login_cookie_of(&response);
    let answer: Value = serde_json::from_str(&body(response).await).unwrap();
    parse_started(answer["url"].as_str().unwrap(), id)
}

/// Starts a Reauth with a session.
async fn start_reauth(env: &Env, session: &str) -> Response {
    send(
        env,
        request(
            Method::POST,
            "/auth/reauth/google",
            &[
                origin_header(),
                ("cookie", format!("{}={session}", team_session::COOKIE_NAME)),
            ],
        ),
    )
    .await
}

async fn callback_with(
    env: &Env,
    started: &Started,
    state: &str,
    cookies: Vec<String>,
) -> Response {
    let uri = format!("/auth/callback/google?code=the-code&state={state}");
    let headers: Vec<(&str, String)> = if cookies.is_empty() {
        vec![]
    } else {
        vec![("cookie", cookies.join("; "))]
    };
    let _ = started;
    send(env, request(Method::GET, &uri, &headers)).await
}

async fn callback(env: &Env, started: &Started) -> Response {
    callback_with(
        env,
        started,
        &started.state,
        vec![format!("{LOGIN_COOKIE}={}", started.id)],
    )
    .await
}

/// Refused: 400 and no session came of it.
fn assert_refused(response: &Response, env: &Env) {
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_value(response, team_session::COOKIE_NAME).is_none());
    assert_eq!(env.state.sessions.len(), 0);
}

fn session_of(response: &Response) -> String {
    set_cookie_value(response, team_session::COOKIE_NAME).expect("a session cookie is set")
}

fn hardened(response: &Response) -> bool {
    let h = response.headers();
    h[header::CACHE_CONTROL] == "no-store"
        && h[header::REFERRER_POLICY] == "no-referrer"
        && h[header::X_FRAME_OPTIONS] == "DENY"
        && h[header::CONTENT_SECURITY_POLICY]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
}

// ===== Sign in =====

#[tokio::test]
async fn a_linked_member_signs_in_and_gets_a_new_session() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let started = start_sign_in(&env).await;
    assert!(started.url.starts_with(&env.fake.issuer));
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/app");
    assert!(hardened(&response));
    let token = session_of(&response);
    assert_eq!(
        env.state.sessions.lookup(&token).as_deref(),
        Some(env.amy.0.id.as_str())
    );
    assert!(env.state.sessions.fresh(&token, REAUTH_WINDOW));
    // The login cookie is cleared, and PKCE S256 held: the verifier the
    // provider was given hashes to the challenge it was sent.
    let cleared = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .any(|v| v.to_str().unwrap().starts_with("__Host-wenlan-login=; "));
    assert!(cleared);
    let verifier = env.fake.verifier.lock().unwrap().clone().unwrap();
    let digest = Sha256::digest(verifier.as_bytes());
    assert_eq!(URL_SAFE_NO_PAD.encode(digest), started.challenge);
    // The session works.
    let me = send(
        &env,
        request(
            Method::GET,
            "/team/v1/me",
            &[("cookie", format!("{}={token}", team_session::COOKIE_NAME))],
        ),
    )
    .await;
    assert_eq!(me.status(), StatusCode::OK);
    assert_eq!(env.state.auth.transactions_len(), 0);
}

#[tokio::test]
async fn a_callback_with_the_wrong_state_is_refused_and_burns_the_login() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let cookie = vec![format!("{LOGIN_COOKIE}={}", started.id)];
    let wrong = callback_with(&env, &started, "not-the-state", cookie).await;
    assert_refused(&wrong, &env);
    assert!(hardened(&wrong));
    // The transaction is gone: the right state cannot finish it now.
    assert_refused(&callback(&env, &started).await, &env);
}

#[tokio::test]
async fn a_login_is_used_once() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    assert_eq!(callback(&env, &started).await.status(), StatusCode::FOUND);
    let again = callback(&env, &started).await;
    assert_eq!(again.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_value(&again, team_session::COOKIE_NAME).is_none());
    assert_eq!(env.state.sessions.len(), 1);
}

#[tokio::test]
async fn a_callback_without_the_login_cookie_is_refused() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback_with(&env, &started, &started.state, vec![]).await;
    assert_refused(&response, &env);
    // Another browser's login id does not do either.
    let other = start_sign_in(&env).await;
    let response = callback_with(
        &env,
        &started,
        &started.state,
        vec![format!("{LOGIN_COOKIE}={}", other.id)],
    )
    .await;
    assert_refused(&response, &env);
}

#[tokio::test]
async fn an_expired_login_is_refused() {
    let env = env_with(Duration::ZERO, BURST, REFILL_PER_SEC, Some(ORIGIN)).await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    assert_refused(&callback(&env, &started).await, &env);
}

#[tokio::test]
async fn an_id_token_that_is_not_right_is_refused() {
    type Change = Box<dyn Fn(&mut Value, &Env)>;
    let cases: Vec<(&str, Change)> = vec![
        ("nonce", Box::new(|c, _| c["nonce"] = json!("other"))),
        ("aud", Box::new(|c, _| c["aud"] = json!("someone-else"))),
        (
            "iss",
            Box::new(|c, _| c["iss"] = json!("http://evil.example")),
        ),
        (
            "expired",
            Box::new(|c, _| {
                let now = chrono::Utc::now().timestamp();
                c["exp"] = json!(now - 3600);
                c["iat"] = json!(now - 7200);
            }),
        ),
        (
            "email_verified",
            Box::new(|c, _| c["email_verified"] = json!(false)),
        ),
        (
            "email_verified missing",
            Box::new(|c, _| {
                c.as_object_mut().unwrap().remove("email_verified");
            }),
        ),
        (
            "nonce missing",
            Box::new(|c, _| {
                c.as_object_mut().unwrap().remove("nonce");
            }),
        ),
    ];
    for (name, change) in cases {
        let env = env().await;
        env.link_directly(&env.amy.0, "s1", "amy@x.org");
        // The same token is accepted when nothing is changed.
        let started = start_sign_in(&env).await;
        env.fake.set(env.claims(&started, "s1", "amy@x.org"));
        assert_eq!(callback(&env, &started).await.status(), StatusCode::FOUND);
        env.state.sessions.delete_member(&env.amy.0.id);

        let started = start_sign_in(&env).await;
        let mut claims = env.claims(&started, "s1", "amy@x.org");
        change(&mut claims, &env);
        env.fake.set(claims);
        let response = callback(&env, &started).await;
        assert_refused(&response, &env);
        assert!(hardened(&response), "{name}");
    }
}

#[tokio::test]
async fn a_token_signed_by_another_key_is_refused() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let started = start_sign_in(&env).await;
    // The provider signs, but a byte of the payload changed afterwards: the
    // signature no longer matches. The fake issues the payload as is, so
    // build the bad token by signing claims whose `sub` differs from the
    // one that is then presented, through a tampering token endpoint.
    let mut claims = env.claims(&started, "s1", "amy@x.org");
    claims["alg_check"] = json!("x");
    let good = env.fake.jwt(&claims);
    let mut parts: Vec<&str> = good.split('.').collect();
    let forged = URL_SAFE_NO_PAD.encode(
        env.claims(&started, "s1", "amy@x.org")
            .to_string()
            .replace("amy@x.org", "bo@x.org"),
    );
    parts[1] = &forged;
    // Check the verifier directly: the forged token fails the signature.
    let meta = env
        .state
        .auth
        .metadata(env.state.auth.provider("google").unwrap(), false)
        .await
        .unwrap();
    let client = client_for(&meta, env.state.auth.provider("google").unwrap(), ORIGIN).unwrap();
    let token: openidconnect::core::CoreIdToken =
        serde_json::from_value(json!(parts.join("."))).unwrap();
    assert!(token
        .claims(
            &client.id_token_verifier(),
            &Nonce::new(started.nonce.clone())
        )
        .is_err());
}

#[tokio::test]
async fn an_account_not_in_the_team_is_told_nothing() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    // A stranger.
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "stranger", "who@x.org"));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(set_cookie_value(&response, team_session::COOKIE_NAME).is_none());
    let stranger_body = body(response).await;
    assert_eq!(stranger_body, "This account is not in this team.");
    // Amy's email under another subject is a stranger too: the email is
    // never the key.
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s-other", "amy@x.org"));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(body(response).await, stranger_body);
    // A removed member is not in the team either.
    team::remove_member(&env.state.team_path, &env.amy.0.id).unwrap();
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(env.state.sessions.len(), 0);
}

#[tokio::test]
async fn a_sign_in_replaces_the_session_the_browser_presented() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let old = env.state.sessions.create(&env.bo.0.id).unwrap();
    let started = start_sign_in(&env).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback_with(
        &env,
        &started,
        &started.state,
        vec![
            format!("{}={old}", team_session::COOKIE_NAME),
            format!("{LOGIN_COOKIE}={}", started.id),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let new = session_of(&response);
    assert_ne!(new, old);
    assert!(env.state.sessions.lookup(&old).is_none());
    assert_eq!(
        env.state.sessions.lookup(&new).as_deref(),
        Some(env.amy.0.id.as_str())
    );
}

#[tokio::test]
async fn one_browser_holds_one_login() {
    let env = env().await;
    let first = start_sign_in(&env).await;
    assert_eq!(env.state.auth.transactions_len(), 1);
    let second = send(
        &env,
        request(
            Method::GET,
            "/auth/start/google?purpose=signin",
            &[("cookie", format!("{LOGIN_COOKIE}={}", first.id))],
        ),
    )
    .await;
    assert_eq!(second.status(), StatusCode::FOUND);
    assert_eq!(env.state.auth.transactions_len(), 1);
    // Only a plain sign-in starts this way.
    let other = send(
        &env,
        request(Method::GET, "/auth/start/google?purpose=link", &[]),
    )
    .await;
    assert_eq!(other.status(), StatusCode::BAD_REQUEST);
}

// ===== Link =====

#[tokio::test]
async fn a_legacy_key_holder_links_an_account_and_a_second_member_cannot_take_it() {
    let env = env().await;
    let started = start_link(&env, &env.amy.1).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let token = session_of(&response);
    assert_eq!(
        env.state.sessions.lookup(&token).as_deref(),
        Some(env.amy.0.id.as_str())
    );
    let amy = env.member(&env.amy.0);
    assert_eq!(amy.identities.len(), 1);
    assert_eq!(amy.identities[0].subject, "s1");
    assert_eq!(amy.identities[0].issuer, env.fake.issuer);
    assert_eq!(amy.identities[0].email.as_deref(), Some("amy@x.org"));

    // Bo starts a Link and the same Google account comes back.
    let started = start_link(&env, &env.bo.1).await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let sessions_before = env.state.sessions.len();
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_value(&response, team_session::COOKIE_NAME).is_none());
    assert_eq!(env.state.sessions.len(), sessions_before);
    assert!(env.member(&env.bo.0).identities.is_empty());
}

#[tokio::test]
async fn starting_a_link_needs_the_own_origin_a_member_and_a_key_that_may() {
    let env = env().await;
    let key = ("authorization", format!("Bearer {}", env.amy.1));
    let post =
        |headers: Vec<(&'static str, String)>| request(Method::POST, "/auth/link/google", &headers);
    // No Origin, another origin, and http: refused.
    for headers in [
        vec![key.clone()],
        vec![key.clone(), ("origin", "https://evil.example".to_string())],
        vec![
            key.clone(),
            ("origin", "http://team.example.com".to_string()),
        ],
    ] {
        let response = send(&env, post(headers)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(hardened(&response));
    }
    // No credential at all.
    let response = send(&env, post(vec![origin_header()])).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    // A tool key never links.
    let (_, tool) = team::issue_tool_key(&env.state.team_path, &env.amy.0.id, "Cursor").unwrap();
    let response = send(
        &env,
        post(vec![
            origin_header(),
            ("authorization", format!("Bearer {tool}")),
        ]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(env.state.auth.transactions_len(), 0);
}

#[tokio::test]
async fn a_browser_session_links_only_after_a_recent_sign_in() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let cookie = |token: &str| ("cookie", format!("{}={token}", team_session::COOKIE_NAME));
    let stale = env.state.sessions.create(&env.amy.0.id).unwrap();
    let response = send(
        &env,
        request(
            Method::POST,
            "/auth/link/google",
            &[origin_header(), cookie(&stale)],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let answer: Value = serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(answer["error"], "reauth_required");
    assert_eq!(env.state.auth.transactions_len(), 0);

    let fresh = env.state.sessions.create_fresh(&env.amy.0.id).unwrap();
    let response = send(
        &env,
        request(
            Method::POST,
            "/auth/link/google",
            &[origin_header(), cookie(&fresh)],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(hardened(&response));
    assert_eq!(env.state.auth.transactions_len(), 1);
}

// ===== Reauth =====

#[tokio::test]
async fn reauth_with_an_account_already_linked_makes_a_fresh_new_session() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let old = env.state.sessions.create(&env.amy.0.id).unwrap();
    assert!(!env.state.sessions.fresh(&old, REAUTH_WINDOW));
    let response = start_reauth(&env, &old).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = login_cookie_of(&response);
    let answer: Value = serde_json::from_str(&body(response).await).unwrap();
    let url = answer["url"].as_str().unwrap();
    // The provider is told to authenticate now.
    assert!(url.contains("max_age=0"), "{url}");
    let started = parse_started(url, id);
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback_with(
        &env,
        &started,
        &started.state,
        vec![
            format!("{}={old}", team_session::COOKIE_NAME),
            format!("{LOGIN_COOKIE}={}", started.id),
        ],
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let new = session_of(&response);
    assert_ne!(new, old);
    assert!(env.state.sessions.lookup(&old).is_none());
    assert!(env.state.sessions.fresh(&new, REAUTH_WINDOW));
}

#[tokio::test]
async fn reauth_with_a_new_account_is_refused() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    let old = env.state.sessions.create(&env.amy.0.id).unwrap();
    for sub in ["attacker", "s-bo"] {
        let response = start_reauth(&env, &old).await;
        let id = login_cookie_of(&response);
        let answer: Value = serde_json::from_str(&body(response).await).unwrap();
        let started = parse_started(answer["url"].as_str().unwrap(), id);
        env.fake.set(env.claims(&started, sub, "amy@x.org"));
        let response = callback_with(
            &env,
            &started,
            &started.state,
            vec![
                format!("{}={old}", team_session::COOKIE_NAME),
                format!("{LOGIN_COOKIE}={}", started.id),
            ],
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{sub}");
        assert!(set_cookie_value(&response, team_session::COOKIE_NAME).is_none());
    }
    // The session stays as it was, not fresh, and nothing was linked.
    assert!(env.state.sessions.lookup(&old).is_some());
    assert!(!env.state.sessions.fresh(&old, REAUTH_WINDOW));
    assert_eq!(env.member(&env.amy.0).identities.len(), 1);
}

#[tokio::test]
async fn reauth_needs_a_fresh_authentication_and_its_own_member_s_session() {
    let env = env().await;
    env.link_directly(&env.amy.0, "s1", "amy@x.org");
    env.link_directly(&env.bo.0, "s-bo", "bo@x.org");
    let old = env.state.sessions.create(&env.amy.0.id).unwrap();
    let begin = || async {
        let response = start_reauth(&env, &old).await;
        let id = login_cookie_of(&response);
        let answer: Value = serde_json::from_str(&body(response).await).unwrap();
        parse_started(answer["url"].as_str().unwrap(), id)
    };
    let cookies = |started: &Started, session: &str| {
        vec![
            format!("{}={session}", team_session::COOKIE_NAME),
            format!("{LOGIN_COOKIE}={}", started.id),
        ]
    };
    // The provider signs in again, but its authentication was ten minutes
    // ago (it ignored max_age): refused.
    let started = begin().await;
    let mut claims = env.claims(&started, "s1", "amy@x.org");
    claims["auth_time"] = json!(chrono::Utc::now().timestamp() - 600);
    env.fake.set(claims);
    let response = callback_with(&env, &started, &started.state, cookies(&started, &old)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // No auth_time at all: refused.
    let started = begin().await;
    let mut claims = env.claims(&started, "s1", "amy@x.org");
    claims.as_object_mut().unwrap().remove("auth_time");
    env.fake.set(claims);
    let response = callback_with(&env, &started, &started.state, cookies(&started, &old)).await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // A browser without the session cannot finish it.
    let started = begin().await;
    env.fake.set(env.claims(&started, "s1", "amy@x.org"));
    let response = callback_with(
        &env,
        &started,
        &started.state,
        vec![format!("{LOGIN_COOKIE}={}", started.id)],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // Nor with another member's session, even with that member's account.
    let bo_session = env.state.sessions.create(&env.bo.0.id).unwrap();
    let started = begin().await;
    env.fake.set(env.claims(&started, "s-bo", "bo@x.org"));
    let response = callback_with(
        &env,
        &started,
        &started.state,
        cookies(&started, &bo_session),
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    // A legacy key cannot start one, and a member with no account cannot.
    let response = send(
        &env,
        request(
            Method::POST,
            "/auth/reauth/google",
            &[
                origin_header(),
                ("authorization", format!("Bearer {}", env.amy.1)),
            ],
        ),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let (cy, _) = team::add_member(&env.state.team_path, "Cy", &[]).unwrap();
    let cy_session = env.state.sessions.create(&cy.id).unwrap();
    let response = start_reauth(&env, &cy_session).await;
    assert_eq!(response.status(), StatusCode::CONFLICT);
}

// ===== Off, limits, headers, configuration =====

#[tokio::test]
async fn sign_in_is_off_without_https_or_a_provider() {
    let routes = [
        (Method::GET, "/auth/start/google?purpose=signin"),
        (Method::GET, "/auth/callback/google?code=c&state=s"),
        (Method::POST, "/auth/link/google"),
        (Method::POST, "/auth/reauth/google"),
    ];
    let http = env_with(
        TRANSACTION_TTL,
        BURST,
        REFILL_PER_SEC,
        Some("http://team.example.com"),
    )
    .await;
    let unset = env_with(TRANSACTION_TTL, BURST, REFILL_PER_SEC, None).await;
    let mut no_provider = env().await;
    no_provider.state.auth = Arc::default();
    for (name, env) in [
        ("http", &http),
        ("no url", &unset),
        ("no provider", &no_provider),
    ] {
        for (method, uri) in &routes {
            let response = send(
                env,
                request(
                    method.clone(),
                    uri,
                    &[
                        origin_header(),
                        ("authorization", format!("Bearer {}", env.amy.1)),
                    ],
                ),
            )
            .await;
            assert_eq!(
                response.status(),
                StatusCode::NOT_FOUND,
                "{name} {method} {uri}"
            );
            assert!(hardened(&response));
        }
        assert_eq!(env.state.auth.transactions_len(), 0);
    }
    // An unknown provider is 404 even when sign-in is on.
    let on = env().await;
    let response = send(
        &on,
        request(Method::GET, "/auth/start/gitlab?purpose=signin", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn the_auth_routes_are_limited_per_peer() {
    let env = env_with(TRANSACTION_TTL, 3.0, 0.0, Some(ORIGIN)).await;
    let from = |addr: &str| {
        let mut req = request(Method::GET, "/auth/start/google?purpose=bad", &[]);
        req.extensions_mut()
            .insert(ConnectInfo::<SocketAddr>(addr.parse().unwrap()));
        req
    };
    for _ in 0..3 {
        let response = send(&env, from("198.51.100.1:1")).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let limited = send(&env, from("198.51.100.1:2")).await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(limited.headers().contains_key(header::RETRY_AFTER));
    assert!(hardened(&limited));
    // Someone else is not locked out by it.
    let other = send(&env, from("198.51.100.2:1")).await;
    assert_eq!(other.status(), StatusCode::BAD_REQUEST);
    // Nor are the rest of the team's routes limited.
    let health = send(&env, request(Method::GET, "/health", &[])).await;
    assert_eq!(health.status(), StatusCode::OK);
}

#[test]
fn an_ipv6_peer_is_limited_by_its_network() {
    let a: IpAddr = "2001:db8::1".parse().unwrap();
    let b: IpAddr = "2001:db8::ffff".parse().unwrap();
    let c: IpAddr = "2001:db9::1".parse().unwrap();
    assert_eq!(limiter_key(a), limiter_key(b));
    assert_ne!(limiter_key(a), limiter_key(c));
    let mapped: IpAddr = "::ffff:192.0.2.1".parse().unwrap();
    assert_eq!(limiter_key(mapped), "192.0.2.1".parse::<IpAddr>().unwrap());
}

#[test]
fn the_login_store_is_bounded() {
    let state = AuthState::default();
    let tx = || Transaction {
        state: "s".into(),
        nonce: "n".into(),
        pkce_verifier: "v".into(),
        provider: "google".into(),
        purpose: Purpose::SignIn,
        created_at: Instant::now(),
        created_wall: Utc::now(),
    };
    for _ in 0..TRANSACTION_CAP + 10 {
        state.put(tx(), None).unwrap();
    }
    assert_eq!(state.transactions_len(), TRANSACTION_CAP);
}

#[test]
fn the_route_table_covers_the_auth_routes() {
    use team::CredentialKind::{BrowserSession, LegacyKey, ToolKey};
    assert_eq!(
        super::super::route_class("/auth/start/google"),
        super::super::RouteClass::AuthPublic
    );
    assert_eq!(
        super::super::route_class("/auth/callback/google"),
        super::super::RouteClass::AuthPublic
    );
    for kind in [LegacyKey, BrowserSession] {
        assert!(super::super::credential_may_reach(
            kind,
            "/auth/link/google"
        ));
    }
    assert!(!super::super::credential_may_reach(
        ToolKey,
        "/auth/link/google"
    ));
    assert!(super::super::credential_may_reach(
        BrowserSession,
        "/auth/reauth/google"
    ));
    assert!(!super::super::credential_may_reach(
        LegacyKey,
        "/auth/reauth/google"
    ));
    assert!(!super::super::credential_may_reach(
        ToolKey,
        "/auth/reauth/google"
    ));
}

#[test]
fn provider_settings_come_from_the_environment_or_the_file_and_the_environment_wins() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(AUTH_FILE);
    let none = |_: &str| None;
    assert!(load_providers(&file, none).is_empty());
    std::fs::write(
        &file,
        r#"{"google": {"client_id": "file-id", "client_secret": "file-secret"}}"#,
    )
    .unwrap();
    let found = load_providers(&file, none);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].client_id, "file-id");
    assert_eq!(found[0].issuer, GOOGLE_ISSUER);
    // The environment wins, per value.
    let env = |name: &str| (name == GOOGLE_CLIENT_ID_ENV).then(|| "env-id".to_string());
    let found = load_providers(&file, env);
    assert_eq!(found[0].client_id, "env-id");
    assert_eq!(found[0].client_secret, "file-secret");
    // Environment alone, and half a configuration is none.
    let both = |name: &str| Some(format!("env-{name}"));
    assert_eq!(load_providers(&dir.path().join("none.json"), both).len(), 2);
    let half = |name: &str| (name == GOOGLE_CLIENT_ID_ENV).then(|| "id".to_string());
    assert!(load_providers(&dir.path().join("none.json"), half).is_empty());
    // A broken file is none; an issuer that is not https is not taken.
    std::fs::write(&file, "{ not json").unwrap();
    assert!(load_providers(&file, none).is_empty());
    std::fs::write(
        &file,
        r#"{"google": {"client_id": "i", "client_secret": "s", "issuer": "http://x"}}"#,
    )
    .unwrap();
    assert_eq!(load_providers(&file, none)[0].issuer, GOOGLE_ISSUER);
    // The secret never shows in a debug print.
    let shown = format!("{:?}", found[0]);
    assert!(!shown.contains("file-secret"), "{shown}");
}

// ===== Invites =====

fn invite_request(token: &str, headers: Vec<(&str, String)>) -> Request<Body> {
    let mut req = Request::builder()
        .method(Method::POST)
        .uri("/auth/invite/google")
        .header("content-type", "application/json");
    for (name, value) in headers {
        req = req.header(name, value);
    }
    let mut req = req
        .body(Body::from(json!({ "token": token }).to_string()))
        .unwrap();
    req.extensions_mut().insert(ConnectInfo::<SocketAddr>(
        "203.0.113.7:4000".parse().unwrap(),
    ));
    req
}

/// Starts signing in with an invite token; the answer is the response.
async fn post_invite(env: &Env, token: &str) -> Response {
    send(env, invite_request(token, vec![origin_header()])).await
}

async fn start_invite(env: &Env, token: &str) -> Started {
    let response = post_invite(env, token).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = login_cookie_of(&response);
    let answer: Value = serde_json::from_str(&body(response).await).unwrap();
    let url = answer["url"].as_str().unwrap().to_string();
    assert!(!url.contains("inv_"), "the token must not leave in a URL");
    parse_started(&url, id)
}

fn invite_member(env: &Env, email: Option<&str>) -> (Member, String) {
    team::create_invited_member(&env.state.team_path, "Dee", email, &[]).unwrap()
}

/// Claims for an invite sign-in, with the account's `hd` when given.
fn invite_claims(env: &Env, started: &Started, sub: &str, email: &str, hd: Option<&str>) -> Value {
    let mut claims = env.claims(started, sub, email);
    if let Some(hd) = hd {
        claims["hd"] = json!(hd);
    }
    claims
}

/// Makes an invite's expiry the past, as the file would say after a week.
fn expire(env: &Env, member: &Member) {
    let path = &env.state.team_path;
    let mut file: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    for m in file["members"].as_array_mut().unwrap() {
        if m["id"] == json!(member.id) {
            m["invite"]["expires_at"] = json!("2020-01-01T00:00:00Z");
        }
    }
    std::fs::write(path, serde_json::to_vec(&file).unwrap()).unwrap();
}

async fn admin_post(env: &Env, owner_key: &str, uri: &str) -> Response {
    send(
        env,
        request(
            Method::POST,
            uri,
            &[("authorization", format!("Bearer {owner_key}"))],
        ),
    )
    .await
}

#[tokio::test]
async fn an_invited_gmail_account_is_linked_and_signed_in() {
    let env = env().await;
    let (dee, token) = invite_member(&env, Some("Dee@gmail.com"));
    let started = start_invite(&env, &token).await;
    env.fake
        .set(invite_claims(&env, &started, "g1", "dee@gmail.com", None));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(response.headers()[header::LOCATION], "/app");
    assert!(hardened(&response));
    let session = session_of(&response);
    assert_eq!(
        env.state.sessions.lookup(&session).as_deref(),
        Some(dee.id.as_str())
    );
    let now = env.member(&dee);
    assert!(now.invite.is_none());
    assert_eq!(now.identities.len(), 1);
    assert_eq!(now.identities[0].subject, "g1");
    // The token is spent: the page is told so before Google is bothered.
    assert_eq!(post_invite(&env, &token).await.status(), StatusCode::GONE);
    // Signing in again is by the account.
    let again = start_sign_in(&env).await;
    env.fake.set(env.claims(&again, "g1", "dee@gmail.com"));
    assert_eq!(callback(&env, &again).await.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn an_invited_company_account_with_a_hosted_domain_is_linked() {
    let env = env().await;
    let (dee, token) = invite_member(&env, Some("dee@corp.example"));
    let started = start_invite(&env, &token).await;
    env.fake.set(invite_claims(
        &env,
        &started,
        "c1",
        "dee@corp.example",
        Some("corp.example"),
    ));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(env.member(&dee).identities.len(), 1);
}

#[tokio::test]
async fn an_untrusted_matching_email_waits_until_the_owner_approves() {
    let env = env().await;
    let (dee, token) = invite_member(&env, Some("dee@corp.example"));
    let started = start_invite(&env, &token).await;
    env.fake.set(invite_claims(
        &env,
        &started,
        "c1",
        "dee@corp.example",
        None,
    ));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(set_cookie_value(&response, team_session::COOKIE_NAME).is_none());
    assert_eq!(env.state.sessions.len(), 0);
    assert!(body(response).await.contains("approve"));
    assert!(env.member(&dee).identities.is_empty());

    // Not yet: the account is not in the team.
    let attempt = start_sign_in(&env).await;
    env.fake.set(env.claims(&attempt, "c1", "dee@corp.example"));
    assert_eq!(
        callback(&env, &attempt).await.status(),
        StatusCode::FORBIDDEN
    );
    // The token cannot be used again to take the place.
    assert_eq!(post_invite(&env, &token).await.status(), StatusCode::GONE);

    let (_, owner_key) = team::owner_key(&env.state.team_path, "Owner").unwrap();
    let uri = format!("/team/v1/admin/members/{}/approve", dee.id);
    let approved = admin_post(&env, &owner_key, &uri).await;
    assert_eq!(approved.status(), StatusCode::OK);

    let attempt = start_sign_in(&env).await;
    env.fake.set(env.claims(&attempt, "c1", "dee@corp.example"));
    let response = callback(&env, &attempt).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    assert_eq!(
        env.state.sessions.lookup(&session_of(&response)).as_deref(),
        Some(dee.id.as_str())
    );
}

#[tokio::test]
async fn the_wrong_email_is_refused_with_a_masked_address_and_the_invite_still_works() {
    let env = env().await;
    let (dee, token) = invite_member(&env, Some("amy.dee@example.com"));
    let started = start_invite(&env, &token).await;
    env.fake
        .set(invite_claims(&env, &started, "e1", "eve@gmail.com", None));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(set_cookie_value(&response, team_session::COOKIE_NAME).is_none());
    let message = body(response).await;
    assert!(message.contains("a***@example.com"), "{message}");
    assert!(!message.contains("amy.dee"), "{message}");
    assert_eq!(env.state.sessions.len(), 0);
    let now = env.member(&dee);
    assert!(now.identities.is_empty() && now.invite.is_some());
    // The right person can still use it.
    let started = start_invite(&env, &token).await;
    env.fake.set(invite_claims(
        &env,
        &started,
        "d1",
        "amy.dee@example.com",
        Some("example.com"),
    ));
    assert_eq!(callback(&env, &started).await.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn an_account_of_another_active_member_is_refused() {
    let env = env().await;
    env.link_directly(&env.amy.0, "g1", "amy@gmail.com");
    let (dee, token) = invite_member(&env, None);
    let started = start_invite(&env, &token).await;
    env.fake
        .set(invite_claims(&env, &started, "g1", "amy@gmail.com", None));
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(env.state.sessions.len(), 0);
    assert!(env.member(&dee).identities.is_empty());
    assert_eq!(env.member(&env.amy.0).identities.len(), 1);
}

#[tokio::test]
async fn an_invite_that_expired_is_gone_at_the_start_and_at_the_end() {
    let env = env().await;
    let (dee, token) = invite_member(&env, None);
    let started = start_invite(&env, &token).await;
    env.fake
        .set(invite_claims(&env, &started, "g1", "dee@gmail.com", None));
    expire(&env, &dee);
    let response = callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(body(response).await.contains("expired"));
    assert!(env.member(&dee).identities.is_empty());
    assert_eq!(post_invite(&env, &token).await.status(), StatusCode::GONE);
}

#[tokio::test]
async fn a_new_invite_stops_the_old_one_at_every_step() {
    let env = env().await;
    let (dee, old) = invite_member(&env, None);
    let started = start_invite(&env, &old).await;
    env.fake
        .set(invite_claims(&env, &started, "g1", "dee@gmail.com", None));
    let (_, owner_key) = team::owner_key(&env.state.team_path, "Owner").unwrap();
    let uri = format!("/team/v1/admin/members/{}/invite", dee.id);
    let response = admin_post(&env, &owner_key, &uri).await;
    assert_eq!(response.status(), StatusCode::OK);
    let answer: wenlan_types::team::TeamInviteLink =
        serde_json::from_str(&body(response).await).unwrap();
    let fresh = answer.link.split('#').nth(1).unwrap().to_string();
    // A sign-in begun with the old token ends in a refusal; a new start too.
    assert_eq!(
        callback(&env, &started).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(post_invite(&env, &old).await.status(), StatusCode::GONE);
    let started = start_invite(&env, &fresh).await;
    env.fake
        .set(invite_claims(&env, &started, "g1", "dee@gmail.com", None));
    assert_eq!(callback(&env, &started).await.status(), StatusCode::FOUND);
}

#[tokio::test]
async fn starting_an_invite_needs_the_exact_origin_and_a_real_token() {
    let env = env().await;
    let (_, token) = invite_member(&env, None);
    // No Origin, another Origin, and the same host over http: all 403.
    for headers in [
        vec![],
        vec![("origin", "https://evil.example".to_string())],
        vec![("origin", "http://team.example.com".to_string())],
    ] {
        let response = send(&env, invite_request(&token, headers)).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert!(set_cookie_value(&response, LOGIN_COOKIE).is_none());
    }
    assert_eq!(env.state.auth.transactions_len(), 0);
    // Not an invite token: a member key, junk, a made-up invite.
    for bad in [
        env.amy.1.clone(),
        "junk".to_string(),
        format!("inv_{}", "0".repeat(64)),
    ] {
        let response = post_invite(&env, &bad).await;
        assert!(
            matches!(
                response.status(),
                StatusCode::BAD_REQUEST | StatusCode::GONE
            ),
            "{bad}"
        );
    }
    assert_eq!(env.state.auth.transactions_len(), 0);
    // The token in a query string is not read.
    let mut req = request(
        Method::POST,
        &format!("/auth/invite/google?token={token}"),
        &[origin_header()],
    );
    *req.body_mut() = Body::from("{}");
    assert_eq!(send(&env, req).await.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn invite_sign_in_is_off_when_sign_in_is_off() {
    let env = env_with(TRANSACTION_TTL, BURST, REFILL_PER_SEC, None).await;
    let (_, token) = invite_member(&env, None);
    let response = send(&env, invite_request(&token, vec![origin_header()])).await;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[test]
fn only_gmail_and_hosted_domains_are_trusted_to_own_their_email() {
    use team::EmailTrust::{Trusted, Untrusted};
    assert_eq!(google_email_trust("a@gmail.com", false), Trusted);
    assert_eq!(google_email_trust("a@GoogleMail.com", false), Trusted);
    assert_eq!(google_email_trust("a@corp.example", true), Trusted);
    assert_eq!(google_email_trust("a@corp.example", false), Untrusted);
    assert_eq!(google_email_trust("a@notgmail.com", false), Untrusted);
    assert_eq!(
        google_email_trust("a@gmail.com.evil.example", false),
        Untrusted
    );
}

// ===== GitHub =====

fn gh_user(id: Value, login: &str) -> Value {
    json!({"id": id, "login": login, "name": "Someone"})
}

fn gh_email(address: &str, verified: bool, primary: bool) -> Value {
    json!({"email": address, "verified": verified, "primary": primary, "visibility": null})
}

/// What a login begun for GitHub sent the browser to; the fake learns the
/// PKCE challenge so it can check the verifier it is given.
fn parse_github_started(env: &Env, url: &str, id: String) -> Started {
    let parsed = url::Url::parse(url).unwrap();
    let get = |k: &str| {
        parsed
            .query_pairs()
            .find(|(n, _)| n == k)
            .map(|(_, v)| v.to_string())
            .unwrap_or_default()
    };
    assert!(url.starts_with(&format!("{}/authorize?", env.gh.base)));
    assert_eq!(get("code_challenge_method"), "S256");
    assert!(!get("code_challenge").is_empty());
    assert_eq!(get("client_id"), "gh-client");
    assert_eq!(get("scope"), "read:user user:email");
    assert_eq!(
        get("redirect_uri"),
        format!("{ORIGIN}/auth/callback/github")
    );
    assert!(parsed.query_pairs().all(|(n, _)| n != "nonce"));
    *env.gh.challenge.lock().unwrap() = Some(get("code_challenge"));
    Started {
        id,
        state: get("state"),
        nonce: String::new(),
        challenge: get("code_challenge"),
        url: url.to_string(),
    }
}

async fn start_github_sign_in(env: &Env) -> Started {
    let response = send(
        env,
        request(Method::GET, "/auth/start/github?purpose=signin", &[]),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let id = login_cookie_of(&response);
    let url = response.headers()[header::LOCATION].to_str().unwrap();
    parse_github_started(env, url, id)
}

async fn start_github_post(env: &Env, uri: &str, headers: &[(&str, String)]) -> Started {
    let response = send(env, request(Method::POST, uri, headers)).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = login_cookie_of(&response);
    let answer: Value = serde_json::from_str(&body(response).await).unwrap();
    parse_github_started(env, answer["url"].as_str().unwrap(), id)
}

async fn start_github_invite(env: &Env, token: &str) -> Started {
    let mut req = invite_request(token, vec![origin_header()]);
    *req.uri_mut() = "/auth/invite/github".parse().unwrap();
    let response = send(env, req).await;
    assert_eq!(response.status(), StatusCode::OK);
    let id = login_cookie_of(&response);
    let answer: Value = serde_json::from_str(&body(response).await).unwrap();
    parse_github_started(env, answer["url"].as_str().unwrap(), id)
}

async fn github_callback_with(
    env: &Env,
    started: &Started,
    code: &str,
    state: &str,
    extra_cookies: Vec<String>,
) -> Response {
    let mut cookies = extra_cookies;
    cookies.push(format!("{LOGIN_COOKIE}={}", started.id));
    send(
        env,
        request(
            Method::GET,
            &format!("/auth/callback/github?code={code}&state={state}"),
            &[("cookie", cookies.join("; "))],
        ),
    )
    .await
}

async fn github_callback(env: &Env, started: &Started) -> Response {
    github_callback_with(env, started, GH_CODE, &started.state, vec![]).await
}

/// The access token is in no header and no body of a response.
async fn assert_no_token_leak(response: Response) {
    let headers = format!("{:?}", response.headers());
    assert!(!headers.contains(GH_ACCESS_TOKEN));
    assert!(!body(response).await.contains(GH_ACCESS_TOKEN));
}

fn github_member(env: &Env, member: &Member, id: &str) {
    team::link_identity(
        &env.state.team_path,
        &member.id,
        Identity {
            issuer: GITHUB_ISSUER.into(),
            subject: id.into(),
            email: None,
            linked_at: String::new(),
        },
    )
    .unwrap();
}

#[tokio::test]
async fn a_github_member_signs_in_by_numeric_id_and_a_rename_changes_nothing() {
    let env = env().await;
    github_member(&env, &env.amy.0, "4242");
    for login in ["amy", "amy-renamed"] {
        env.gh.set(
            gh_user(json!(4242), login),
            json!([gh_email("a@x.org", true, true)]),
        );
        let started = start_github_sign_in(&env).await;
        let response = github_callback(&env, &started).await;
        assert_eq!(response.status(), StatusCode::FOUND, "{login}");
        assert_eq!(response.headers()[header::LOCATION], "/app");
        assert!(hardened(&response));
        let token = session_of(&response);
        assert_eq!(
            env.state.sessions.lookup(&token).as_deref(),
            Some(env.amy.0.id.as_str())
        );
        assert_no_token_leak(response).await;
    }
    // The token request carried the verifier, the secret, the callback and
    // the code; the API calls carried the token as a Bearer and a
    // User-Agent.
    let form = env.gh.token_forms.lock().unwrap()[0].clone();
    assert_eq!(form["client_id"], "gh-client");
    assert_eq!(form["client_secret"], "gh-secret");
    assert_eq!(
        form["redirect_uri"],
        format!("{ORIGIN}/auth/callback/github")
    );
    assert!(!form["code_verifier"].is_empty());
    let calls = env.gh.api_calls.lock().unwrap().clone();
    assert_eq!(calls[0].0, "/user");
    assert_eq!(calls[1].0, "/user/emails");
    for (_, auth, agent) in &calls {
        assert_eq!(
            auth.as_deref(),
            Some(format!("Bearer {GH_ACCESS_TOKEN}").as_str())
        );
        assert_eq!(agent.as_deref(), Some("wenlan-team-server"));
    }
    // An account that is not a member is told nothing more.
    env.gh.set(gh_user(json!(9), "amy"), json!([]));
    let started = start_github_sign_in(&env).await;
    let response = github_callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn a_github_login_is_refused_for_a_bad_state_code_or_verifier() {
    let env = env().await;
    github_member(&env, &env.amy.0, "4242");
    env.gh.set(gh_user(json!(4242), "amy"), json!([]));
    // A state that is not the transaction's.
    let started = start_github_sign_in(&env).await;
    let response = github_callback_with(&env, &started, GH_CODE, "other-state", vec![]).await;
    assert_refused(&response, &env);
    assert!(env.gh.token_forms.lock().unwrap().is_empty());
    // A code GitHub does not accept.
    let started = start_github_sign_in(&env).await;
    let response = github_callback_with(&env, &started, "wrong", &started.state, vec![]).await;
    assert_refused(&response, &env);
    // A verifier that does not hash to the challenge sent (the fake treats
    // a missing one the same way).
    let started = start_github_sign_in(&env).await;
    *env.gh.challenge.lock().unwrap() = Some("not-the-challenge".into());
    let response = github_callback(&env, &started).await;
    assert_refused(&response, &env);
    assert_no_token_leak(response).await;
    // The login is gone after a refusal.
    *env.gh.challenge.lock().unwrap() = Some(started.challenge.clone());
    assert_refused(&github_callback(&env, &started).await, &env);
}

#[tokio::test]
async fn a_github_answer_without_a_numeric_id_is_refused() {
    let env = env().await;
    github_member(&env, &env.amy.0, "4242");
    for user in [
        json!({"login": "amy"}),
        json!({"id": "4242", "login": "amy"}),
        json!({"id": null, "login": "amy"}),
        json!({"id": 0, "login": "amy"}),
        json!({"id": -5, "login": "amy"}),
    ] {
        env.gh.set(user.clone(), json!([]));
        let started = start_github_sign_in(&env).await;
        let response = github_callback(&env, &started).await;
        assert_refused(&response, &env);
    }
}

#[tokio::test]
async fn a_github_account_with_no_verified_email_can_still_sign_in() {
    let env = env().await;
    github_member(&env, &env.amy.0, "7");
    env.gh.set(
        gh_user(json!(7), "amy"),
        json!([gh_email("a@x.org", false, true)]),
    );
    let started = start_github_sign_in(&env).await;
    assert_eq!(
        github_callback(&env, &started).await.status(),
        StatusCode::FOUND
    );
}

#[tokio::test]
async fn an_invite_is_matched_by_any_verified_github_email() {
    let env = env().await;
    let (dee, token) = invite_member(&env, Some("Dee@Work.example"));
    env.gh.set(
        gh_user(json!(555), "dee"),
        json!([
            gh_email("dee@home.example", true, true),
            gh_email("dee@work.example", true, false),
        ]),
    );
    let started = start_github_invite(&env, &token).await;
    assert!(!started.url.contains("inv_"));
    let response = github_callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let session = session_of(&response);
    assert_eq!(
        env.state.sessions.lookup(&session).as_deref(),
        Some(dee.id.as_str())
    );
    let now = env.member(&dee);
    assert!(now.invite.is_none(), "trusted: linked at once, no approval");
    assert_eq!(now.identities.len(), 1);
    assert_eq!(now.identities[0].issuer, GITHUB_ISSUER);
    assert_eq!(now.identities[0].subject, "555");
    // The shown address is the primary verified one.
    assert_eq!(now.identities[0].email.as_deref(), Some("dee@home.example"));
}

#[tokio::test]
async fn an_unverified_github_email_does_not_match_an_invite() {
    let env = env().await;
    let (dee, token) = invite_member(&env, Some("dee@work.example"));
    env.gh.set(
        gh_user(json!(555), "dee"),
        json!([
            gh_email("dee@home.example", true, true),
            gh_email("dee@work.example", false, false),
        ]),
    );
    let started = start_github_invite(&env, &token).await;
    let response = github_callback(&env, &started).await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    let text = body(response).await;
    assert!(text.contains("d***@work.example"), "{text}");
    assert!(!text.contains("dee@work.example"));
    // Nothing changed, and the invite still works for the right account.
    let now = env.member(&dee);
    assert!(now.identities.is_empty() && now.invite.is_some());
    env.gh.set(
        gh_user(json!(555), "dee"),
        json!([gh_email("dee@work.example", true, true)]),
    );
    let started = start_github_invite(&env, &token).await;
    assert_eq!(
        github_callback(&env, &started).await.status(),
        StatusCode::FOUND
    );
}

#[tokio::test]
async fn an_invite_without_an_email_works_for_a_github_account_with_none() {
    let env = env().await;
    let (dee, token) = invite_member(&env, None);
    env.gh.set(gh_user(json!(8), "dee"), json!([]));
    let started = start_github_invite(&env, &token).await;
    assert_eq!(
        github_callback(&env, &started).await.status(),
        StatusCode::FOUND
    );
    assert_eq!(env.member(&dee).identities[0].subject, "8");
}

#[tokio::test]
async fn a_key_holder_links_github_and_another_member_cannot_take_it() {
    let env = env().await;
    env.gh.set(
        gh_user(json!(31), "amy"),
        json!([gh_email("a@x.org", true, true)]),
    );
    let bearer = |key: &str| ("authorization", format!("Bearer {key}"));
    let started = start_github_post(
        &env,
        "/auth/link/github",
        &[origin_header(), bearer(&env.amy.1)],
    )
    .await;
    assert_eq!(
        github_callback(&env, &started).await.status(),
        StatusCode::FOUND
    );
    let linked = env.member(&env.amy.0);
    assert_eq!(linked.identities.len(), 1);
    assert_eq!(linked.identities[0].issuer, GITHUB_ISSUER);
    assert_eq!(linked.identities[0].subject, "31");
    // Bo cannot link the same account.
    let started = start_github_post(
        &env,
        "/auth/link/github",
        &[origin_header(), bearer(&env.bo.1)],
    )
    .await;
    assert_eq!(
        github_callback(&env, &started).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert!(env.member(&env.bo.0).identities.is_empty());
}

#[tokio::test]
async fn github_reauth_needs_an_account_already_linked_to_this_member() {
    let env = env().await;
    github_member(&env, &env.amy.0, "31");
    let old = env.state.sessions.create(&env.amy.0.id).unwrap();
    let cookie = format!("{}={old}", team_session::COOKIE_NAME);
    // An account that is not Amy's is refused and the session is kept.
    env.gh.set(gh_user(json!(99), "mallory"), json!([]));
    let started = start_github_post(
        &env,
        "/auth/reauth/github",
        &[origin_header(), ("cookie", cookie.clone())],
    )
    .await;
    // GitHub has no max_age: none is invented.
    assert!(!started.url.contains("max_age"));
    let response = github_callback_with(
        &env,
        &started,
        GH_CODE,
        &started.state,
        vec![cookie.clone()],
    )
    .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert!(set_cookie_value(&response, team_session::COOKIE_NAME).is_none());
    assert!(env.state.sessions.lookup(&old).is_some());
    assert!(!env.state.sessions.fresh(&old, REAUTH_WINDOW));
    // Amy's own account makes a fresh new session.
    env.gh.set(gh_user(json!(31), "amy"), json!([]));
    let started = start_github_post(
        &env,
        "/auth/reauth/github",
        &[origin_header(), ("cookie", cookie.clone())],
    )
    .await;
    let response =
        github_callback_with(&env, &started, GH_CODE, &started.state, vec![cookie]).await;
    assert_eq!(response.status(), StatusCode::FOUND);
    let new = session_of(&response);
    assert_ne!(new, old);
    assert!(env.state.sessions.lookup(&old).is_none());
    assert!(env.state.sessions.fresh(&new, REAUTH_WINDOW));
}

#[test]
fn github_settings_come_from_the_environment_or_the_file_and_the_environment_wins() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join(AUTH_FILE);
    let none = |_: &str| None;
    std::fs::write(
        &file,
        r#"{"github": {"client_id": "file-id", "client_secret": "file-secret", "issuer": "https://evil.example"}}"#,
    )
    .unwrap();
    let found = load_providers(&file, none);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "github");
    assert_eq!(found[0].client_id, "file-id");
    assert_eq!(found[0].issuer, GITHUB_ISSUER);
    let Kind::GitHub(urls) = &found[0].kind else {
        panic!("a GitHub provider");
    };
    assert_eq!(urls.authorize, "https://github.com/login/oauth/authorize");
    assert_eq!(urls.token, "https://github.com/login/oauth/access_token");
    assert_eq!(urls.api, "https://api.github.com");
    // The environment wins, per value, and does not disturb Google.
    let env = |name: &str| (name == GITHUB_CLIENT_ID_ENV).then(|| "env-id".to_string());
    let found = load_providers(&file, env);
    assert_eq!(found[0].client_id, "env-id");
    assert_eq!(found[0].client_secret, "file-secret");
    // Both providers at once; either alone suffices; half is none.
    let both = |name: &str| Some(format!("env-{name}"));
    let found = load_providers(&dir.path().join("none.json"), both);
    assert_eq!(
        found.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["google", "github"]
    );
    let half = |name: &str| (name == GITHUB_CLIENT_SECRET_ENV).then(|| "s".to_string());
    assert!(load_providers(&dir.path().join("none.json"), half).is_empty());
    assert!(!format!("{:?}", found[1]).contains("env-WENLAN_TEAM_GITHUB_CLIENT_SECRET"));
}

#[tokio::test]
async fn github_alone_turns_sign_in_on_and_google_routes_stay_404() {
    let env = env().await;
    let github_only = AuthState::new(vec![Provider::github_with_urls(
        "gh-client",
        "gh-secret",
        env.gh.urls(),
    )]);
    let state = TeamState {
        auth: Arc::new(github_only),
        ..env.state.clone()
    };
    assert_eq!(offered_names(&state).await, ["github"]);
    let response = team_router(state.clone())
        .oneshot(request(
            Method::GET,
            "/auth/start/google?purpose=signin",
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let response = team_router(state)
        .oneshot(request(
            Method::GET,
            "/auth/start/github?purpose=signin",
            &[],
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FOUND);
}

async fn offered_names(state: &TeamState) -> Vec<String> {
    offered(state).await
}

#[test]
fn the_route_table_and_join_page_cover_github() {
    use team::CredentialKind::{BrowserSession, LegacyKey, ToolKey};
    assert_eq!(
        super::super::route_class("/auth/callback/github"),
        super::super::RouteClass::AuthPublic
    );
    assert_eq!(
        super::super::route_class("/auth/invite/github"),
        super::super::RouteClass::AuthPublic
    );
    assert!(super::super::credential_may_reach(
        LegacyKey,
        "/auth/link/github"
    ));
    assert!(!super::super::credential_may_reach(
        ToolKey,
        "/auth/link/github"
    ));
    assert!(super::super::credential_may_reach(
        BrowserSession,
        "/auth/reauth/github"
    ));
    assert!(!super::super::credential_may_reach(
        LegacyKey,
        "/auth/reauth/github"
    ));
    let page = super::super::render_join_page(
        None,
        &super::super::JOIN_PAGE_EN,
        "n",
        &["google".to_string(), "github".to_string()],
    );
    assert!(page.contains("id=\"accept-github\""));
    assert!(page.contains("const providers = [\"google\",\"github\"];"));
    assert!(page.contains("\"sign_in_github\":\"Sign in with GitHub\""));
}

#[tokio::test]
async fn providers_lists_what_is_on_and_is_an_empty_list_when_off() {
    let env = env().await;
    async fn ask(env: &Env) -> Response {
        send(env, request(Method::GET, "/auth/providers", &[])).await
    }
    let response = ask(&env).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(hardened(&response));
    let listed: wenlan_types::team::TeamProviders =
        serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(listed.providers, ["google", "github"]);
    assert_eq!(listed.team, None);
    team::rename_team(&env.state.team_path, "Acme").unwrap();
    let response = ask(&env).await;
    let listed: wenlan_types::team::TeamProviders =
        serde_json::from_str(&body(response).await).unwrap();
    assert_eq!(listed.team.as_deref(), Some("Acme"));

    // Off for every reason sign-in can be off: an empty list, not a 404, and
    // it needs no credential.
    let http = env_with(
        TRANSACTION_TTL,
        BURST,
        REFILL_PER_SEC,
        Some("http://team.example.com"),
    )
    .await;
    let mut no_provider = env_with(TRANSACTION_TTL, BURST, REFILL_PER_SEC, Some(ORIGIN)).await;
    no_provider.state.auth = Arc::default();
    for off in [&http, &no_provider] {
        let response = ask(off).await;
        assert_eq!(response.status(), StatusCode::OK);
        let listed: wenlan_types::team::TeamProviders =
            serde_json::from_str(&body(response).await).unwrap();
        assert!(listed.providers.is_empty());
    }
    assert_eq!(
        super::super::route_class("/auth/providers"),
        super::super::RouteClass::AuthPublic
    );
}

#[tokio::test]
async fn providers_is_rate_limited_like_the_rest_of_sign_in() {
    let env = env_with(TRANSACTION_TTL, 2.0, 0.0, Some(ORIGIN)).await;
    let mut statuses = Vec::new();
    for _ in 0..4 {
        let response = send(&env, request(Method::GET, "/auth/providers", &[])).await;
        statuses.push(response.status());
    }
    assert_eq!(statuses[0], StatusCode::OK);
    assert_eq!(statuses[3], StatusCode::TOO_MANY_REQUESTS);
}

#[test]
fn sign_in_is_configured_only_with_a_provider_and_an_https_address() {
    let dir = tempfile::tempdir().unwrap();
    let team_path = dir.path().join(team::TEAM_FILE);
    let none = |_: &str| None;
    let github = |name: &str| {
        (name == GITHUB_CLIENT_ID_ENV || name == GITHUB_CLIENT_SECRET_ENV).then(|| "x".to_string())
    };
    let https = Some("https://team.example.com");
    // No provider anywhere: off, whatever the address.
    assert!(!configured_with(&team_path, https, none));
    assert!(configured_with(&team_path, https, github));
    assert!(configured_with(
        &team_path,
        Some(" HTTPS://team.example.com "),
        github
    ));
    assert!(!configured_with(
        &team_path,
        Some("http://team.example.com"),
        github
    ));
    assert!(!configured_with(&team_path, None, github));
}
