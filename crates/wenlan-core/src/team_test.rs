// SPDX-License-Identifier: Apache-2.0
use super::*;

fn access(space: &str, access: TeamAccess) -> TeamSpaceAccess {
    TeamSpaceAccess {
        space: space.into(),
        access,
        id: None,
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
    assert_eq!(amy.me(&[]).spaces.len(), 2);
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

#[test]
fn the_team_has_one_id_and_the_name_its_owner_gave_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    assert_eq!(read_team(&path).unwrap().info(), None);
    let info = team_info(&path).unwrap();
    assert_eq!(info.name, None);
    // The id is kept: asked again, or after the team is named, it is the same.
    assert_eq!(team_info(&path).unwrap().id, info.id);
    let named = rename_team(&path, "  Acme ").unwrap();
    assert_eq!(named.id, info.id);
    assert_eq!(named.name.as_deref(), Some("Acme"));
    assert!(rename_team(&path, " ").is_err());

    // Every member's Wenlan learns the team from who they are.
    let (_, key) = add_member(&path, "Amy", &[]).unwrap();
    let me = authenticate(&path, &key).unwrap().unwrap().me(&[]);
    assert_eq!(me.team, Some(named));
    assert_eq!(me.role, TeamRole::Member);
}

#[test]
fn a_server_nobody_sits_at_has_one_owner_who_can_edit_every_hosted_space() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    assert!(!has_owner(&path).unwrap());
    let (owner, first) = owner_key(&path, "Owner").unwrap();
    assert_eq!(owner.role, TeamRole::Owner);
    assert!(has_owner(&path).unwrap());

    let found = authenticate(&path, &first).unwrap().unwrap();
    assert_eq!(found.access("Product"), Some(TeamAccess::Write));
    let me = found.me(&[
        ("Product".to_string(), PathBuf::from("/x/team-spaces/0a1b")),
        ("Design".to_string(), PathBuf::from("/x/team-spaces/2c3d")),
    ]);
    assert_eq!(me.role, TeamRole::Owner);
    // A hosted Space carries its folder's id, which a rename keeps.
    let with_id = |space: &str, id: &str| TeamSpaceAccess {
        id: Some(id.to_string()),
        ..access(space, TeamAccess::Write)
    };
    assert_eq!(
        me.spaces,
        vec![with_id("Design", "2c3d"), with_id("Product", "0a1b")]
    );

    // A new owner link is the same owner with a new key; the old one stops.
    let (again, second) = owner_key(&path, "Someone else").unwrap();
    assert_eq!(again.id, owner.id);
    assert_eq!(read_team(&path).unwrap().members.len(), 1);
    assert!(authenticate(&path, &first).unwrap().is_none());
    assert!(authenticate(&path, &second).unwrap().is_some());

    // The owner is never removed.
    assert!(remove_member(&path, &owner.id).is_err());
    assert!(authenticate(&path, &second).unwrap().is_some());

    // A key the owner's own Wenlan made, given to the server by its SHA-256
    // only, is the same owner's; the earlier key stops.
    let made_by_owner = format!("wlt_{}", "b2".repeat(32));
    let hash = key_sha256(&made_by_owner);
    assert!(is_key_sha256(&hash));
    assert!(!is_owner_hash(&path, &hash).unwrap());
    let same = set_owner_hash(&path, "Owner", &hash).unwrap();
    assert_eq!(same.id, owner.id);
    assert!(is_owner_hash(&path, &hash).unwrap());
    assert!(authenticate(&path, &made_by_owner).unwrap().is_some());
    assert!(authenticate(&path, &second).unwrap().is_none());
    // The key itself, or anything but a hash, is refused.
    assert!(set_owner_hash(&path, "Owner", &made_by_owner).is_err());
    assert!(set_owner_hash(&path, "Owner", &hash.to_uppercase()).is_err());
}

#[test]
fn a_space_that_stops_being_hosted_takes_its_access_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (bob, key) = add_member(&path, "Bob", &[access("Finance", TeamAccess::Read)]).unwrap();
    set_everyone(&path, "Finance", Some(TeamAccess::Write)).unwrap();
    let found = authenticate(&path, &key).unwrap().unwrap();
    assert_eq!(found.access("Finance"), Some(TeamAccess::Read));

    forget_space_access(&path, "Finance").unwrap();
    // A new "Finance" later is not open to Bob because the old one was.
    let found = authenticate(&path, &key).unwrap().unwrap();
    assert_eq!(found.id, bob.id);
    assert_eq!(found.access("Finance"), None);
    // Nothing to forget, or no team file at all: nothing happens.
    forget_space_access(&path, "Finance").unwrap();
    forget_space_access(&dir.path().join("none.json"), "Finance").unwrap();
    assert!(!dir.path().join("none.json").exists());
}

#[test]
fn a_renamed_space_keeps_its_access_and_its_share() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (_, key) = add_member(&path, "Bob", &[access("Finance", TeamAccess::Read)]).unwrap();
    set_everyone(&path, "Finance", Some(TeamAccess::Write)).unwrap();
    rename_space_access(&path, "Finance", "Money").unwrap();
    let found = authenticate(&path, &key).unwrap().unwrap();
    assert_eq!(found.access("Money"), Some(TeamAccess::Read));
    assert_eq!(found.access("Finance"), None, "the old name opens nothing");
    assert_eq!(
        read_team(&path).unwrap().everyone["Money"],
        TeamAccess::Write
    );

    let settings = dir.path().join("shared-spaces.json");
    let folder = new_hosted_folder(&settings);
    let money = Scope::Space("Money".into());
    shared_space::join_at(&settings, &money, &folder, "Server").unwrap();
    assert!(shared_space::rename_share(&settings, "Money", "Cash").unwrap());
    assert!(shared_space::share_settings_at(&settings, &money)
        .unwrap()
        .is_none());
    let cash = shared_space::share_settings_at(&settings, &Scope::Space("Cash".into()))
        .unwrap()
        .unwrap();
    assert_eq!(cash.folder, folder.to_string_lossy());
    assert_eq!(hosted_space_id(&folder).unwrap().len(), 32);
    // Not shared: nothing moves. Onto a share: refused.
    assert!(!shared_space::rename_share(&settings, "Other", "Else").unwrap());
    shared_space::join_at(&settings, &money, &folder, "Server").unwrap();
    assert!(shared_space::rename_share(&settings, "Money", "Cash").is_err());
}

#[test]
fn the_listener_address_and_invite_address_are_kept_until_turned_off() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    set_listen(&path, Some("0.0.0.0:7879"), Some("http://100.80.1.2:7879/")).unwrap();
    let file = read_team(&path).unwrap();
    assert_eq!(file.listen.as_deref(), Some("0.0.0.0:7879"));
    assert_eq!(file.url.as_deref(), Some("http://100.80.1.2:7879"));
    assert!(set_listen(&path, Some("not an address"), None).is_err());

    // Off: the invite address stays for when it is on again.
    set_listen(&path, None, None).unwrap();
    let file = read_team(&path).unwrap();
    assert_eq!(file.listen, None);
    assert_eq!(file.url.as_deref(), Some("http://100.80.1.2:7879"));
    set_url(&path, Some("https://wiki.acme.com")).unwrap();
    assert_eq!(
        read_team(&path).unwrap().url.as_deref(),
        Some("https://wiki.acme.com")
    );
}

#[test]
fn key_hashes_match_only_when_identical() {
    let hash = key_hash("wl_team_example");
    assert!(same_hash(&hash, &key_hash("wl_team_example")));
    assert!(!same_hash(&hash, &key_hash("wl_team_other")));
    assert!(!same_hash(&hash, &hash[..hash.len() - 1]));
    assert!(!same_hash(&hash, ""));
}

// ===== Format 2: tool keys, identities, invites =====

fn bak(path: &Path) -> PathBuf {
    path.with_file_name(V1_BACKUP_FILE)
}

fn raw(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

#[test]
fn a_format_1_file_is_read_and_rewritten_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, _) = add_member(&path, "Amy", &[access("Design", TeamAccess::Write)]).unwrap();
    // Writes that need nothing new keep format 1 and make no backup.
    new_member_key(&path, &amy.id).unwrap();
    rename_team(&path, "Team").unwrap();
    assert!(!bak(&path).exists());
    let before = std::fs::read_to_string(&path).unwrap();
    assert_eq!(raw(&path)["format"], 1);
    for field in ["identities", "keys", "invite"] {
        assert!(!before.contains(field), "{field}");
    }
    // Read and write back through the types: byte-identical.
    let file = read_team(&path).unwrap();
    assert_eq!(file.format, FORMAT);
    assert_eq!(serde_json::to_string_pretty(&file).unwrap(), before);
    // A hand-written format 1 file (as today's code wrote it) reads too.
    let old = r#"{"format":1,"id":"t","members":[{"id":"a","name":"Amy","key_sha256":"00","spaces":{},"added_at":"2026-01-01T00:00:00Z"}]}"#;
    std::fs::write(&path, old).unwrap();
    let file = read_team(&path).unwrap();
    assert_eq!(file.members[0].key_sha256.as_deref(), Some("00"));
    assert_eq!(serde_json::to_string(&file).unwrap(), old);
}

#[test]
fn the_first_tool_key_makes_format_2_and_keeps_a_format_1_backup_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, _) = add_member(&path, "Amy", &[]).unwrap();
    let v1 = std::fs::read(&path).unwrap();
    assert_eq!(raw(&path)["format"], 1);

    let (id, key) = issue_tool_key(&path, &amy.id, "Cursor").unwrap();
    assert!(wenlan_types::team::is_team_key(&key));
    assert!(key.starts_with("wlt_") && key.len() == 68);
    assert_eq!(raw(&path)["format"], 2);
    assert_eq!(std::fs::read(bak(&path)).unwrap(), v1);
    // Only the hash is kept.
    assert!(!std::fs::read_to_string(&path).unwrap().contains(&key));

    // A later write never replaces the backup.
    issue_tool_key(&path, &amy.id, "Claude").unwrap();
    revoke_tool_key(&path, &amy.id, &id).unwrap();
    assert_eq!(std::fs::read(bak(&path)).unwrap(), v1);
    // This code reads format 2.
    assert_eq!(read_team(&path).unwrap().format, FORMAT_V2);
}

#[test]
fn an_existing_backup_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, _) = add_member(&path, "Amy", &[]).unwrap();
    std::fs::write(bak(&path), "earlier").unwrap();
    issue_tool_key(&path, &amy.id, "Cursor").unwrap();
    assert_eq!(std::fs::read_to_string(bak(&path)).unwrap(), "earlier");
}

#[test]
fn a_member_without_a_legacy_key_makes_format_2_and_today_s_reader_refuses_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, _) = add_member(&path, "Amy", &[]).unwrap();
    change_team(&path, |file| {
        file.members[0].key_sha256 = None;
        Ok(())
    })
    .unwrap();
    assert_eq!(raw(&path)["format"], 2);
    assert!(bak(&path).exists());
    let file = read_team(&path).unwrap();
    assert_eq!(file.members[0].id, amy.id);
    assert!(file.members[0].key_sha256.is_none());

    // Today's code reads a member as a struct with a required `key_sha256`
    // string, and reads the whole file before looking at the version. One
    // keyless member therefore makes the whole list unreadable to it, which
    // is why the format moves to 2 and the old file is kept.
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct V1Member {
        id: String,
        key_sha256: String,
    }
    #[derive(serde::Deserialize)]
    #[allow(dead_code)]
    struct V1File {
        format: u32,
        members: Vec<V1Member>,
    }
    let error = serde_json::from_slice::<V1File>(&std::fs::read(&path).unwrap())
        .err()
        .expect("a format 1 reader cannot parse a keyless member")
        .to_string();
    assert!(error.contains("missing field `key_sha256`"), "{error}");
    // The backup is still readable by it.
    assert!(serde_json::from_slice::<V1File>(&std::fs::read(bak(&path)).unwrap()).is_ok());
}

#[test]
fn a_newer_format_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    std::fs::write(&path, r#"{"format":3,"members":[]}"#).unwrap();
    assert!(read_team(&path).is_err());
}

#[test]
fn a_tool_key_authenticates_until_revoked_or_the_member_is_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, legacy) = add_member(&path, "Amy", &[access("Design", TeamAccess::Write)]).unwrap();
    let (bo, _) = add_member(&path, "Bo", &[]).unwrap();
    let (id, key) = issue_tool_key(&path, &amy.id, "Cursor").unwrap();
    let (id2, key2) = issue_tool_key(&path, &bo.id, "Cursor").unwrap();

    let found = authenticate(&path, &key).unwrap().unwrap();
    assert_eq!(found.member.id, amy.id);
    assert_eq!(found.credential, Credential::ToolKey { id: id.clone() });
    assert_eq!(found.access("Design"), Some(TeamAccess::Write));
    let by_legacy = authenticate(&path, &legacy).unwrap().unwrap();
    assert_eq!(by_legacy.credential, Credential::LegacyKey);

    revoke_tool_key(&path, &amy.id, &id).unwrap();
    assert!(authenticate(&path, &key).unwrap().is_none());
    assert!(authenticate(&path, &legacy).unwrap().is_some());
    assert!(revoke_tool_key(&path, &amy.id, "nope").is_err());
    // Another member's key id is not theirs to revoke.
    assert!(revoke_tool_key(&path, &amy.id, &id2).is_err());

    assert!(authenticate(&path, &key2).unwrap().is_some());
    remove_member(&path, &bo.id).unwrap();
    assert!(authenticate(&path, &key2).unwrap().is_none());
    assert!(issue_tool_key(&path, &bo.id, "Late").is_err());
}

#[test]
fn reset_compromised_kills_every_credential_and_keeps_the_member() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, old) = add_member(&path, "Amy", &[access("Design", TeamAccess::Write)]).unwrap();
    let (_, tool1) = issue_tool_key(&path, &amy.id, "One").unwrap();
    let (_, tool2) = issue_tool_key(&path, &amy.id, "Two").unwrap();
    change_team(&path, |file| {
        let member = &mut file.members[0];
        member.identities.push(Identity {
            issuer: "https://accounts.google.com".into(),
            subject: "123".into(),
            email: Some("a@example.com".into()),
            linked_at: now(),
        });
        member.invite = Some(Invite {
            token_sha256: "ab".repeat(32),
            email: None,
            expires_at: now(),
        });
        Ok(())
    })
    .unwrap();

    let (member, fresh) = reset_compromised(&path, &amy.id).unwrap();
    assert_eq!(member.id, amy.id);
    assert!(wenlan_types::team::is_team_key(&fresh));
    for dead in [&old, &tool1, &tool2] {
        assert!(authenticate(&path, dead).unwrap().is_none());
    }
    let found = authenticate(&path, &fresh).unwrap().unwrap();
    assert_eq!(found.member.id, amy.id);
    assert_eq!(found.credential, Credential::LegacyKey);
    assert_eq!(found.access("Design"), Some(TeamAccess::Write));
    let listed = read_team(&path).unwrap();
    let stored = &listed.members[0];
    assert!(stored.identities.is_empty() && stored.invite.is_none());
    assert!(stored.keys.iter().all(|key| key.revoked_at.is_some()));
    assert!(reset_compromised(&path, "missing").is_err());
}

#[test]
fn reset_compromised_works_on_the_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (owner, old) = owner_key(&path, "Owner").unwrap();
    let (_, tool) = issue_tool_key(&path, &owner.id, "Cursor").unwrap();
    let (again, fresh) = reset_compromised(&path, &owner.id).unwrap();
    assert_eq!(again.id, owner.id);
    assert_eq!(again.role, TeamRole::Owner);
    assert!(authenticate(&path, &old).unwrap().is_none());
    assert!(authenticate(&path, &tool).unwrap().is_none());
    let found = authenticate(&path, &fresh).unwrap().unwrap();
    assert_eq!(found.role, TeamRole::Owner);
}

#[test]
fn member_by_id_sees_a_removed_member_as_removed() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(TEAM_FILE);
    let (amy, _) = add_member(&path, "Amy", &[]).unwrap();
    assert!(member_by_id(&path, &amy.id).unwrap().unwrap().is_active());
    assert!(member_by_id(&path, "nobody").unwrap().is_none());
    remove_member(&path, &amy.id).unwrap();
    assert!(!member_by_id(&path, &amy.id).unwrap().unwrap().is_active());
    assert_eq!(
        Credential::BrowserSession.kind(),
        CredentialKind::BrowserSession
    );
}
