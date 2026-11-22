// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

fn version_path(kind: &str, bytes: &[u8]) -> String {
    format!(
        "members/m1/{kind}/page_1/{}.wlv",
        version::sha256_hex(bytes)
    )
}

#[test]
fn pages_and_memories_are_kept_once_and_listed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let page = b"a page version".to_vec();
    let memory = b"a memory version".to_vec();
    let space = br#"{"devices":[]}"#.to_vec();
    assert_eq!(
        put(root, &version_path("pages", &page), &page).unwrap(),
        Ok(())
    );
    assert_eq!(
        put(root, &version_path("memories", &memory), &memory).unwrap(),
        Ok(())
    );
    assert_eq!(put(root, SPACE_FILE, &space).unwrap(), Ok(()));
    // The same version again changes nothing.
    assert_eq!(
        put(root, &version_path("pages", &page), &page).unwrap(),
        Ok(())
    );
    let mut want = vec![
        SPACE_FILE.to_string(),
        version_path("memories", &memory),
        version_path("pages", &page),
    ];
    want.sort();
    assert_eq!(list(root).unwrap(), want);
    assert_eq!(
        std::fs::read(local_path(root, &version_path("pages", &page)).unwrap()).unwrap(),
        page
    );
}

#[test]
fn space_json_is_replaced() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), SPACE_FILE, br#"{"devices":["a"]}"#)
        .unwrap()
        .unwrap();
    put(dir.path(), SPACE_FILE, br#"{"devices":["a","b"]}"#)
        .unwrap()
        .unwrap();
    assert_eq!(
        std::fs::read(dir.path().join(SPACE_FILE)).unwrap(),
        br#"{"devices":["a","b"]}"#
    );
}

#[test]
fn a_file_the_library_does_not_hold_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bytes = b"x".to_vec();
    let sha = version::sha256_hex(&bytes);
    for bad in [
        format!("../{sha}.wlv"),
        format!("members/m1/chats/page_1/{sha}.wlv"),
        format!("members/../pages/page_1/{sha}.wlv"),
        format!("members/m1/pages/page_1/../{sha}.wlv"),
        "members/m1/pages/page_1/not-a-version.txt".to_string(),
        "team.json".to_string(),
    ] {
        assert_eq!(
            put(root, &bad, &bytes).unwrap(),
            Err(Refused::Path),
            "{bad}"
        );
    }
    let wrong = format!("members/m1/pages/page_1/{}.wlv", version::sha256_hex(b"y"));
    assert_eq!(put(root, &wrong, &bytes).unwrap(), Err(Refused::Mismatch));
    assert_eq!(
        put(root, SPACE_FILE, b"not json").unwrap(),
        Err(Refused::NotJson)
    );
    let big = vec![b'a'; MAX_FILE_BYTES + 1];
    assert_eq!(
        put(root, &version_path("pages", &big), &big).unwrap(),
        Err(Refused::TooLarge)
    );
    assert!(list(root).unwrap().is_empty());
}
