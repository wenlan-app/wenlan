// SPDX-License-Identifier: Apache-2.0
use super::*;

fn space(devices: &[&str], main: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "format": 1,
        "space": "Library",
        "library": true,
        "members": [{"id": "m1", "name": "Qi"}],
        "devices": devices.iter().map(|d| serde_json::json!({"id": d, "name": d})).collect::<Vec<_>>(),
        "main_device": main,
    }))
    .unwrap()
}

#[test]
fn space_json_follows_the_side_that_changed() {
    let old = space(&["a"], "a");
    let new = space(&["a", "b"], "a");
    let last = version::sha256_hex(&old);
    assert_eq!(space_step(None, None, ""), SpaceStep::Same);
    assert_eq!(space_step(None, Some(&old), ""), SpaceStep::Take);
    assert_eq!(space_step(Some(&old), None, ""), SpaceStep::Send);
    assert_eq!(space_step(Some(&old), Some(&old), &last), SpaceStep::Same);
    // This computer did not change it since: the cloud's is newer.
    assert_eq!(space_step(Some(&old), Some(&new), &last), SpaceStep::Take);
    // The cloud did not: this computer's is.
    assert_eq!(space_step(Some(&new), Some(&old), &last), SpaceStep::Send);
}

#[test]
fn both_changed_keeps_every_computer_and_the_clouds_main_one() {
    let here = space(&["a", "c"], "c");
    let cloud = space(&["a", "b"], "b");
    let SpaceStep::Both(bytes) = space_step(Some(&here), Some(&cloud), "neither") else {
        panic!("both changed");
    };
    let merged: SpaceFile = serde_json::from_slice(&bytes).unwrap();
    let ids: Vec<&str> = merged.devices.iter().map(|d| d.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(merged.main_device, "b");
    assert_eq!(merged.members.len(), 1);
    // Something that is not a library's file: the cloud's is kept.
    assert_eq!(
        space_step(Some(b"not json"), Some(&cloud), "neither"),
        SpaceStep::Take
    );
}

#[test]
fn only_a_librarys_files_are_listed_or_named() {
    let dir = tempfile::tempdir().unwrap();
    let folder = dir.path();
    let bytes = b"a version";
    let sha = version::sha256_hex(bytes);
    let page = format!("members/m1/pages/p1/{sha}.wlv");
    let memory = format!("members/m1/memories/x1/{sha}.wlv");
    for path in [&page, &memory] {
        assert!(store(path, &local_path(folder, path).unwrap(), bytes).unwrap());
    }
    // Already here: never replaced.
    assert!(!store(&page, &local_path(folder, &page).unwrap(), bytes).unwrap());
    // Bytes that do not match the name are not kept.
    let wrong = format!("members/m1/pages/p2/{sha}.wlv");
    assert!(!store(&wrong, &local_path(folder, &wrong).unwrap(), b"other").unwrap());
    std::fs::write(folder.join(SPACE_FILE), space(&["a"], "a")).unwrap();
    std::fs::create_dir_all(folder.join("members/m1/chats/p1")).unwrap();
    std::fs::write(folder.join(format!("members/m1/chats/p1/{sha}.wlv")), bytes).unwrap();
    std::fs::write(folder.join("members/m1/pages/p1/notes.txt"), b"x").unwrap();
    let mut want = vec![SPACE_FILE.to_string(), memory, page];
    want.sort();
    assert_eq!(list_files(folder).unwrap(), want);
    for bad in [
        format!("../{sha}.wlv"),
        format!("members/../pages/p1/{sha}.wlv"),
        format!("members/m1/pages/p1/../{sha}.wlv"),
        format!("members/m1/chats/p1/{sha}.wlv"),
        "team.json".to_string(),
    ] {
        assert_eq!(local_path(folder, &bad), None, "{bad}");
    }
}
