// SPDX-License-Identifier: AGPL-3.0-only
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};

/// A store whose clock moves only when the test says.
fn store() -> (SessionStore, Arc<AtomicU64>) {
    let base = Instant::now();
    let offset = Arc::new(AtomicU64::new(0));
    let seen = offset.clone();
    let clock = move || base + Duration::from_secs(seen.load(Ordering::SeqCst));
    (SessionStore::with_clock(Arc::new(clock)), offset)
}

const DAY: u64 = 24 * 60 * 60;

#[test]
fn a_token_is_32_random_bytes_and_only_its_hash_is_kept() {
    let (store, _) = store();
    let a = store.create("m1").unwrap();
    let b = store.create("m1").unwrap();
    assert_eq!(a.len(), 64);
    assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
    assert_ne!(a, b);
    assert_eq!(store.lookup(&a).as_deref(), Some("m1"));
    assert!(store.sessions().keys().all(|key| hex::encode(key) != a));
    assert!(store.lookup("nope").is_none());
    assert!(store.lookup(&"0".repeat(64)).is_none());
}

#[test]
fn an_idle_session_ends_but_use_keeps_it() {
    let (store, clock) = store();
    let token = store.create("m1").unwrap();
    for _ in 0..3 {
        clock.fetch_add(29 * DAY, Ordering::SeqCst);
        assert!(store.lookup(&token).is_some());
    }
    // 116 days in, but never idle 30: the 90-day limit ended it.
    clock.fetch_add(29 * DAY, Ordering::SeqCst);
    assert!(store.lookup(&token).is_none());
    assert_eq!(store.len(), 0);

    let idle = store.create("m1").unwrap();
    clock.fetch_add(30 * DAY, Ordering::SeqCst);
    assert!(store.lookup(&idle).is_none());
    assert_eq!(store.len(), 0);
}

#[test]
fn a_session_ends_ninety_days_after_it_was_made() {
    let (store, clock) = store();
    let token = store.create("m1").unwrap();
    for _ in 0..3 {
        clock.fetch_add(29 * DAY, Ordering::SeqCst);
        assert!(store.lookup(&token).is_some());
    }
    // Day 87 now; day 90 is the limit.
    clock.fetch_add(2 * DAY, Ordering::SeqCst);
    assert!(store.lookup(&token).is_some());
    clock.fetch_add(DAY, Ordering::SeqCst);
    assert!(store.lookup(&token).is_none());
    assert_eq!(store.len(), 0);
}

#[test]
fn delete_and_delete_member_end_sessions() {
    let (store, _) = store();
    let a = store.create("m1").unwrap();
    let b = store.create("m1").unwrap();
    let c = store.create("m2").unwrap();
    store.delete(&a);
    assert!(store.lookup(&a).is_none());
    assert!(store.lookup(&b).is_some());
    store.delete_member("m1");
    assert!(store.lookup(&b).is_none());
    assert!(store.lookup(&c).is_some());
}

#[test]
fn a_member_at_the_cap_loses_their_least_recently_used_session() {
    let (store, clock) = store();
    let other = store.create("m2").unwrap();
    let tokens: Vec<String> = (0..PER_MEMBER_CAP)
        .map(|_| {
            clock.fetch_add(1, Ordering::SeqCst);
            store.create("m1").unwrap()
        })
        .collect();
    // The oldest is used, so the second oldest is now the least recent.
    clock.fetch_add(1, Ordering::SeqCst);
    assert!(store.lookup(&tokens[0]).is_some());
    clock.fetch_add(1, Ordering::SeqCst);
    let newest = store.create("m1").unwrap();
    assert!(store.lookup(&tokens[1]).is_none());
    assert!(store.lookup(&tokens[0]).is_some());
    assert!(store.lookup(&newest).is_some());
    assert!(store.lookup(&other).is_some());
    assert_eq!(store.len(), PER_MEMBER_CAP + 1);
}

#[test]
fn the_total_cap_evicts_the_least_recently_used_and_expired_ones_free_room() {
    let (store, clock) = store();
    {
        let mut sessions = store.sessions();
        let now = (store.clock)();
        for i in 0..TOTAL_CAP {
            sessions.insert(
                hash(&format!("t{i}")),
                Session {
                    member_id: format!("m{i}"),
                    created: now,
                    last_used: now + Duration::from_secs(i as u64),
                    reauthed: None,
                },
            );
        }
    }
    clock.fetch_add(TOTAL_CAP as u64 + 1, Ordering::SeqCst);
    let token = store.create("new").unwrap();
    assert_eq!(store.len(), TOTAL_CAP);
    assert!(store.sessions().get(&hash("t0")).is_none());
    assert!(store.sessions().get(&hash("t1")).is_some());
    assert!(store.lookup(&token).is_some());
    // Everything old ends; a new one finds the store nearly empty.
    clock.fetch_add(31 * DAY, Ordering::SeqCst);
    store.create("again").unwrap();
    assert_eq!(store.len(), 1);
}

#[test]
fn the_cookie_has_the_host_prefix_attributes_and_no_domain() {
    let cookie = set_cookie(&"a".repeat(64));
    assert!(cookie.starts_with("__Host-wenlan-session="));
    for part in ["HttpOnly", "Secure", "SameSite=Lax", "Path=/"] {
        assert!(cookie.split("; ").any(|p| p == part), "{part}");
    }
    assert!(!cookie.to_lowercase().contains("domain"));
    let clear = clear_cookie();
    assert!(clear.ends_with("Max-Age=0"));
    assert!(clear.contains("HttpOnly; Secure; SameSite=Lax; Path=/"));
}

#[test]
fn the_cookie_token_is_read_from_the_cookie_header() {
    let mut headers = HeaderMap::new();
    assert_eq!(cookie_token(&headers), None);
    headers.append(
        header::COOKIE,
        "a=1; __Host-wenlan-session=abc; b=2".parse().unwrap(),
    );
    assert_eq!(cookie_token(&headers).as_deref(), Some("abc"));
    assert!(has_cookie(&headers));
    headers.append(header::COOKIE, "__Host-wenlan-session=def".parse().unwrap());
    assert_eq!(cookie_token(&headers), None);
    assert!(has_cookie(&headers));
    let mut other = HeaderMap::new();
    other.append(header::COOKIE, "wenlan-session=abc".parse().unwrap());
    assert!(!has_cookie(&other));
}

#[test]
fn a_session_is_fresh_for_the_window_after_a_sign_in_and_never_without_one() {
    let (store, clock) = store();
    let plain = store.create("m1").unwrap();
    let signed_in = store.create_fresh("m1").unwrap();
    assert!(!store.fresh(&plain, REAUTH_WINDOW));
    assert!(store.fresh(&signed_in, REAUTH_WINDOW));
    clock.fetch_add(4 * 60, Ordering::SeqCst);
    assert!(store.fresh(&signed_in, REAUTH_WINDOW));
    clock.fetch_add(60, Ordering::SeqCst);
    assert!(
        !store.fresh(&signed_in, REAUTH_WINDOW),
        "five minutes is over"
    );
    // Using the session does not make it fresh again.
    assert!(store.lookup(&signed_in).is_some());
    assert!(!store.fresh(&signed_in, REAUTH_WINDOW));
    assert!(!store.fresh("nope", REAUTH_WINDOW));
    store.delete(&plain);
    assert!(!store.fresh(&plain, REAUTH_WINDOW));
}
