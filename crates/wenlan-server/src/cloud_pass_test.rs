// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

#[test]
fn a_pass_is_taken_only_by_the_key_of_its_own_account() {
    let entrance = PassKey::random().unwrap();
    let (alice, bob) = (AccountId::new(), AccountId::new());
    let alices = entrance.for_account(&alice);
    let pass = alices.sign_until(&alice, 1_000 + 30);
    assert_eq!(
        alices.verify_at(&pass, 1_000),
        Some((alice.clone(), Via::Browser))
    );
    // Bob's Wenlan has Bob's key: Alice's pass is not his.
    assert_eq!(entrance.for_account(&bob).verify_at(&pass, 1_000), None);
    // A pass Alice's key makes for Bob is still refused by Bob's.
    let forged = alices.sign_until(&bob, 1_000 + 30);
    assert_eq!(entrance.for_account(&bob).verify_at(&forged, 1_000), None);
}

#[test]
fn an_expired_altered_or_too_long_pass_is_refused() {
    let key = PassKey::random().unwrap();
    let alice = AccountId::new();
    let pass = key.sign_until(&alice, 1_000);
    assert_eq!(key.verify_at(&pass, 1_001), None);
    let long = key.sign_until(&alice, 1_000 + PASS_TTL.as_secs() + 1);
    assert_eq!(key.verify_at(&long, 1_000), None);
    let mut altered = pass.clone();
    altered.replace_range(
        pass.len() - 1..,
        if pass.ends_with('0') { "1" } else { "0" },
    );
    assert_eq!(key.verify_at(&altered, 999), None);
    // How the request came is signed too.
    let computer = key.sign_via_until(&alice, Via::Computer, 1_000);
    assert_eq!(
        key.verify_at(&computer, 999),
        Some((alice.clone(), Via::Computer))
    );
    let claimed = computer.replace(".computer.", ".browser.");
    assert_eq!(key.verify_at(&claimed, 999), None);
    for bad in ["", "v1", "v2.x.1.00", &format!("{pass}.extra")] {
        assert_eq!(key.verify_at(bad, 999), None, "{bad:?}");
    }
}

#[test]
fn a_kept_key_is_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.key");
    let made = PassKey::read_or_create(&path).unwrap();
    let again = PassKey::read_or_create(&path).unwrap();
    let alice = AccountId::new();
    assert_eq!(
        again.verify(&made.sign(&alice)),
        Some((alice, Via::Browser))
    );
    std::fs::write(&path, "not hex").unwrap();
    assert!(PassKey::read_or_create(&path).is_err());
}
