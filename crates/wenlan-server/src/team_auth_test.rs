// SPDX-License-Identifier: AGPL-3.0-only
//! Sign-in against a fake OpenID Connect provider: a local server with
//! discovery, signing keys and a token endpoint, whose id tokens are signed
//! with a throwaway RSA key (a test fixture, used nowhere else).

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

struct Env {
    _dir: tempfile::TempDir,
    state: TeamState,
    fake: Arc<Fake>,
    amy: (Member, String),
    bo: (Member, String),
}

async fn env() -> Env {
    env_with(TRANSACTION_TTL, BURST, REFILL_PER_SEC, Some(ORIGIN)).await
}

async fn env_with(ttl: Duration, burst: f64, refill: f64, url: Option<&str>) -> Env {
    let fake = serve_fake().await;
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
    let auth = AuthState::new(vec![Provider::new(
        "google",
        &fake.issuer,
        CLIENT_ID,
        "client-secret",
    )])
    .tuned(ttl, burst, refill)
    .with_http(http);
    Env {
        state: TeamState {
            settings_path: dir.path().join("settings.json"),
            team_path,
            daemon_url: "http://127.0.0.1:9".into(),
            mcp: Arc::default(),
            sessions: Arc::default(),
            auth: Arc::new(auth),
        },
        _dir: dir,
        fake,
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
        request(Method::GET, "/auth/start/github?purpose=signin", &[]),
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
    assert_eq!(load_providers(&dir.path().join("none.json"), both).len(), 1);
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
