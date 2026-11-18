// SPDX-License-Identifier: AGPL-3.0-only
use std::collections::BTreeMap;

use wenlan_core::team::Member;
use wenlan_types::team::{TeamAccess, TeamRole};

use super::{access, allowed, load, Action};

fn member(
    role: TeamRole,
    removed: bool,
    own: &[(&str, TeamAccess)],
    everyone: &[(&str, TeamAccess)],
) -> Member {
    let map = |list: &[(&str, TeamAccess)]| -> BTreeMap<String, TeamAccess> {
        list.iter().map(|(s, a)| (s.to_string(), *a)).collect()
    };
    Member {
        id: "m-1".into(),
        name: "Amy".into(),
        key_sha256: Some(String::new()),
        identities: Vec::new(),
        keys: Vec::new(),
        invite: None,
        spaces: map(own),
        added_at: "2026-11-17T00:00:00Z".into(),
        removed_at: removed.then(|| "2026-11-17T01:00:00Z".into()),
        role,
        everyone: map(everyone),
        team: None,
    }
}

#[test]
fn the_rules_load_and_pass_the_schema() {
    load().expect("policy.cedar validates against schema.cedarschema");
}

/// The rules answer exactly as the hand-written checks did: the owner may
/// edit every hosted Space; a member what they were given (their own entry
/// winning over everyone's), only where it is hosted; a removed member
/// nothing.
#[test]
fn the_rules_answer_as_the_old_checks_for_every_case() {
    use TeamAccess::{Read, Write};
    let grants: [Option<TeamAccess>; 3] = [None, Some(Read), Some(Write)];
    let mut cases = 0;
    for role in [TeamRole::Owner, TeamRole::Member] {
        for removed in [false, true] {
            for own in grants {
                for everyone in grants {
                    for hosted in [false, true] {
                        let own_list: Vec<(&str, TeamAccess)> =
                            own.map(|a| ("Plans", a)).into_iter().collect();
                        let everyone_list: Vec<(&str, TeamAccess)> =
                            everyone.map(|a| ("Plans", a)).into_iter().collect();
                        let m = member(role, removed, &own_list, &everyone_list);
                        let old = if hosted { m.access("Plans") } else { None };
                        assert_eq!(
                            access(&m, "Plans", hosted),
                            old,
                            "role {role:?} removed {removed} own {own:?} everyone {everyone:?} hosted {hosted}"
                        );
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 72);
}

#[test]
fn a_member_is_not_given_another_space() {
    let m = member(
        TeamRole::Member,
        false,
        &[("Plans", TeamAccess::Write)],
        &[("Design", TeamAccess::Read)],
    );
    assert_eq!(access(&m, "Sales", true), None);
    assert_eq!(access(&m, "Design", true), Some(TeamAccess::Read));
    assert_eq!(access(&m, "Plans", true), Some(TeamAccess::Write));
}

#[test]
fn only_an_active_owner_manages_the_team() {
    assert!(allowed(
        &member(TeamRole::Owner, false, &[], &[]),
        Action::Manage,
        None,
        false
    ));
    assert!(!allowed(
        &member(TeamRole::Owner, true, &[], &[]),
        Action::Manage,
        None,
        false
    ));
    assert!(!allowed(
        &member(
            TeamRole::Member,
            false,
            &[("Plans", TeamAccess::Write)],
            &[]
        ),
        Action::Manage,
        None,
        false
    ));
}

#[test]
fn a_space_name_that_looks_like_a_rule_is_only_a_name() {
    let name = r#"Plans" || true || "x"#;
    let m = member(TeamRole::Member, false, &[(name, TeamAccess::Read)], &[]);
    assert_eq!(access(&m, name, true), Some(TeamAccess::Read));
    assert_eq!(access(&m, "Plans", true), None);
}
