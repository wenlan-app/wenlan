// SPDX-License-Identifier: Apache-2.0
use super::*;

fn access(space: &str, access: TeamAccess) -> TeamSpaceAccess {
    TeamSpaceAccess {
        space: space.into(),
        access,
    }
}

#[test]
fn a_key_finds_its_member_until_they_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, amy_key) = add_member(&path, "Amy", &[access("Design", TeamAccess::Write)]).unwrap();
    let (bo, bo_key) = add_member(&path, " Bo ", &[access("Design", TeamAccess::Read)]).unwrap();
    assert_eq!(bo.name, "Bo");
    assert!(wenlan_types::team::is_team_key(&amy_key));

    let found = authenticate(&path, &amy_key).unwrap().unwrap();
    assert_eq!(found.id, amy.id);
    assert_eq!(found.access("Design"), Some(TeamAccess::Write));
    assert_eq!(found.access("Other"), None);
    assert_eq!(
        authenticate(&path, &bo_key)
            .unwrap()
            .unwrap()
            .access("Design"),
        Some(TeamAccess::Read)
    );
    assert!(authenticate(&path, &new_key()).unwrap().is_none());
    assert!(authenticate(&path, "not a key").unwrap().is_none());

    // The file keeps only hashes.
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains(&amy_key));

    remove_member(&path, &amy.id).unwrap();
    assert!(authenticate(&path, &amy_key).unwrap().is_none());
    let listed = read_team(&path).unwrap();
    assert!(listed
        .members
        .iter()
        .any(|m| m.id == amy.id && m.removed_at.is_some()));
    assert!(update_member(&path, &amy.id, Some("Amy 2"), None).is_err());
}

#[test]
fn a_new_key_replaces_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, old) = add_member(&path, "Amy", &[]).unwrap();
    let (_, new) = new_member_key(&path, &amy.id).unwrap();
    assert!(authenticate(&path, &old).unwrap().is_none());
    assert_eq!(authenticate(&path, &new).unwrap().unwrap().id, amy.id);
    let changed = update_member(
        &path,
        &amy.id,
        Some("Amy Chen"),
        Some(&[access("Plans", TeamAccess::Write)]),
    )
    .unwrap();
    assert_eq!(changed.name, "Amy Chen");
    assert_eq!(changed.access("Plans"), Some(TeamAccess::Write));
    assert!(add_member(&path, "  ", &[]).is_err());
}

#[test]
fn only_version_files_and_space_json_are_served() {
    let folder = Path::new("/srv/team/space");
    assert_eq!(
        served_path(folder, "space.json"),
        Some(folder.join("space.json"))
    );
    let sha = "a".repeat(64);
    assert!(served_path(folder, &format!("members/m1/pages/p1/{sha}.md")).is_some());
    assert!(served_path(folder, &format!("members/m1/pages/p1/{sha}.wlv")).is_some());
    for bad in [
        "../team.json",
        "members/../../x/pages/p/a.md",
        &format!("members/m1/pages/p1/{sha}.txt"),
        &format!("members/m1/other/p1/{sha}.md"),
        "shared-spaces.json",
    ] {
        assert!(served_path(folder, bad).is_none(), "{bad}");
    }
}

#[test]
fn members_of_a_space_are_listed_in_its_space_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let folder = dir.path().join("space");
    let host = SpaceMember {
        id: "host".into(),
        name: "Server".into(),
    };
    shared_space::ensure_space_file(&folder, &Scope::Space("Design".into()), "Design", &host)
        .unwrap();
    let (amy, _) = add_member(&path, "Amy", &[access("Design", TeamAccess::Write)]).unwrap();
    add_member(&path, "Cy", &[access("Other", TeamAccess::Write)]).unwrap();
    let members = read_team(&path).unwrap().members;
    list_members_in_space_file(&folder, "Design", &members).unwrap();
    let file = shared_space::read_space_file(&folder).unwrap().unwrap();
    let names: Vec<&str> = file.members.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Server", "Amy"]);
    assert_eq!(file.members[1].id, amy.id);
    assert_eq!(list_folder_files(&folder), ["space.json"]);
}

#[test]
fn a_space_for_everyone_reaches_every_member_now_and_later() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (_, amy_key) = add_member(&path, "Amy", &[access("Design", TeamAccess::Write)]).unwrap();
    let (_, bo_key) = add_member(&path, "Bo", &[access("Handbook", TeamAccess::Write)]).unwrap();
    set_everyone(&path, "Handbook", Some(TeamAccess::Read)).unwrap();
    let (cy, cy_key) = add_member(&path, "Cy", &[]).unwrap();
    assert_eq!(cy.access("Handbook"), Some(TeamAccess::Read));

    let amy = authenticate(&path, &amy_key).unwrap().unwrap();
    assert_eq!(amy.access("Handbook"), Some(TeamAccess::Read));
    assert_eq!(amy.me().spaces.len(), 2);
    // A member's own entry wins over everyone's.
    let bo = authenticate(&path, &bo_key).unwrap().unwrap();
    assert_eq!(bo.access("Handbook"), Some(TeamAccess::Write));
    // Only their own Spaces are written as theirs.
    assert!(amy
        .to_wire()
        .spaces
        .iter()
        .all(|entry| entry.space == "Design"));
    assert_eq!(
        authenticate(&path, &cy_key)
            .unwrap()
            .unwrap()
            .access("Handbook"),
        Some(TeamAccess::Read)
    );

    set_everyone(&path, "Handbook", None).unwrap();
    assert_eq!(
        authenticate(&path, &amy_key)
            .unwrap()
            .unwrap()
            .access("Handbook"),
        None
    );
    assert_eq!(
        authenticate(&path, &bo_key)
            .unwrap()
            .unwrap()
            .access("Handbook"),
        Some(TeamAccess::Write)
    );
}
