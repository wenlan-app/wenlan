// SPDX-License-Identifier: Apache-2.0

use super::super::tests::{fixture_rule_renderer, fixture_skill_renderer, Fixture};
use super::*;
use wenlan_types::personal_rules::{PersonalRuleEdit, PersonalRuleLayer};
use wenlan_types::rulesync::SharedSourceOffRequest;

const CLAUDE_RULES: &str = "claudecode";

#[test]
fn personal_rule_proposer_uses_the_session_tool_when_no_agent_name_is_sent() {
    let mut headers = HeaderMap::new();
    headers.insert(
        wenlan_types::SESSION_HEADER,
        "codex:synthetic".parse().unwrap(),
    );
    assert_eq!(proposer(&headers).unwrap().as_deref(), Some("codex"));
}

#[tokio::test]
async fn personal_rule_apply_rejects_named_and_session_ai_callers() {
    use wenlan_types::requests::{CALLER_HEADER, PERSON_CALLER_VALUE};

    let fixture = adopted_fixture();
    let before = std::fs::read(fixture.setup.data.join(storage::MANIFEST)).unwrap();
    let state = std::sync::Arc::new(tokio::sync::RwLock::new(
        crate::state::ServerState::default(),
    ));
    let mut person = HeaderMap::new();
    person.insert(CALLER_HEADER, PERSON_CALLER_VALUE.parse().unwrap());
    assert!(require_local_person(&person).is_ok());
    for (header, value) in [
        ("x-agent-name", "codex"),
        (wenlan_types::SESSION_HEADER, "codex:fixture"),
    ] {
        let mut headers = person.clone();
        headers.insert(header, value.parse().unwrap());
        let result = super::apply(
            State(state.clone()),
            headers,
            Json(PersonalRuleApplyRequest {
                plan_id: "fixture".into(),
            }),
        )
        .await;
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains("only the person using the daemon"),
            "{error}"
        );
    }
    assert_eq!(
        std::fs::read(fixture.setup.data.join(storage::MANIFEST)).unwrap(),
        before
    );
}

fn adopted_fixture() -> Fixture {
    let fixture = Fixture::new();
    fixture.adopt_from_plan();
    fixture
}

#[test]
fn source_free_setup_status_is_off_without_opening_the_source() {
    let fixture = Fixture::new();
    let source = fixture.setup.home.join(SHARED_RULES);
    std::fs::remove_file(&source).unwrap();
    std::fs::create_dir(&source).unwrap();
    let status = fixture.setup.status().unwrap();
    assert!(!status.adopted);
    assert!(!status.shared_source_following);
    assert!(source.is_dir());
}

#[test]
fn personal_rule_show_keeps_legacy_tool_only_overrides_visible_or_refuses_ambiguity() {
    let fixture = adopted_fixture();
    let mut manifest = fixture.setup.load().unwrap().unwrap();
    let tool = "codexcli";
    let base = manifest.master.base_rule_text(tool, &[]);
    let appended = format!("{base}A previously accepted tool-only rule.\n");
    manifest.master.overrides.insert(
        tool.into(),
        LocalText {
            base: base.clone(),
            edited: appended.clone(),
        },
    );
    let layer = PersonalRuleLayer::Tool { tool: tool.into() };
    let shown = show_layer(&manifest, &layer).unwrap();
    assert!(shown.text.contains("A previously accepted tool-only rule."));
    assert!(manifest.master.overrides.contains_key(tool));
    let normalized = materialize_tool_overrides(&manifest.master);
    assert!(!normalized.overrides.contains_key(tool));
    assert_eq!(normalized.rule_text(tool, &[]).unwrap(), appended);

    manifest.master.overrides.get_mut(tool).unwrap().edited =
        base.replace("One rule.", "A tool-specific replacement.");
    let refused = show_layer(&manifest, &layer).unwrap_err();
    assert!(refused.to_string().contains("kept unchanged"));
    assert!(materialize_tool_overrides(&manifest.master)
        .overrides
        .contains_key(tool));
}

#[test]
fn personal_rule_plan_waits_for_an_unconfirmed_scope_edit() {
    let fixture = adopted_fixture();
    let codex = super::super::tests::CODEX;
    fixture.write(
        codex,
        &fixture
            .text(codex)
            .replace("One rule.", "Pending shared edit."),
    );
    let report = fixture.sync_with("");
    assert!(!report.proposals.is_empty(), "{report:?}");
    let manifest = fixture.setup.load().unwrap().unwrap();
    assert!(manifest.current_scope_reviews().next().is_some());
    let layer = PersonalRuleLayer::Tool {
        tool: "codexcli".into(),
    };
    let shown = show_layer(&manifest, &layer).unwrap();
    assert!(!shown.text.contains("Pending shared edit."));
    let error = fixture
        .setup
        .prepare_rule_plan(
            &manifest,
            &[PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text: "A new shared rule.\n".into(),
            }],
            &[],
            None,
            &fixture_rule_renderer,
        )
        .err()
        .expect("a pending scope review must block a plan");
    assert!(error.to_string().contains("waiting for review"));
    let before = std::fs::read(fixture.setup.data.join(storage::MANIFEST)).unwrap();
    let error = fixture.setup.restore().unwrap_err();
    assert!(error.to_string().contains("waiting for review"));
    assert_eq!(
        std::fs::read(fixture.setup.data.join(storage::MANIFEST)).unwrap(),
        before
    );
}

#[test]
fn personal_rule_plan_and_source_off_preserve_reviewed_tool_additions_on_setup_restore() {
    use super::super::tests::{accept_local_rule_additions, CLAUDE, CODEX};

    let fixture = Fixture::new();
    let original = "# Codex notes\r\n\r\nAn original rule.  \r\n";
    let additions = "\r\nSecond rule added.  \r\nFirst rule added.\nSecond rule added.\r\n";
    fixture.write(CODEX, original);
    let claude_original = fixture.text(CLAUDE);
    fixture.adopt_from_plan();
    accept_local_rule_additions(&fixture, additions);

    let preview = fixture.setup.preview_source_off().unwrap();
    fixture
        .setup
        .turn_source_off(&SharedSourceOffRequest {
            plan_id: preview.plan_id,
            token: preview.token,
        })
        .unwrap();
    let retired_source = fixture.setup.home.join(SHARED_RULES);
    let marker = "Retired source stays untouched.\n";
    std::fs::write(&retired_source, marker).unwrap();

    let master = fixture.setup.load().unwrap().unwrap().master;
    let plan = propose(
        &fixture,
        vec![
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text: format!("{}A later shared plan.\n", master.shared),
            },
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Tool {
                    tool: "codexcli".into(),
                },
                text: format!("{}A later overlay plan.\n", master.overlays["codexcli"]),
            },
        ],
    );
    apply(&fixture, &plan).unwrap();
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(fixture.text(CODEX), format!("{original}{additions}"));
    assert_eq!(fixture.text(CLAUDE), claude_original);
    assert_eq!(std::fs::read_to_string(retired_source).unwrap(), marker);
    let saved = fixture.setup.load().unwrap().unwrap();
    assert!(saved.master.overlays["codexcli"].contains("A later overlay plan."));
}

#[test]
fn personal_rule_plan_deletion_does_not_restore_a_removed_tool_addition() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let original = fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)].clone();
    let addition = "A reviewed local addition.\n";
    accept_local_rule_additions(&fixture, addition);
    let master = fixture.setup.load().unwrap().unwrap().master;
    let after = master.overlays["codexcli"].replace(addition, "");
    assert_ne!(after, master.overlays["codexcli"]);
    let plan = propose(
        &fixture,
        vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: "codexcli".into(),
            },
            text: after,
        }],
    );
    apply(&fixture, &plan).unwrap();
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(fixture.text(CODEX), text(&original).unwrap());
}

#[test]
fn personal_rule_plan_refuses_an_ambiguous_duplicate_donor_deletion() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let line = "A repeated local rule.\n";
    accept_local_rule_additions(&fixture, &format!("{line}{line}"));
    let before = fixture.setup.load().unwrap().unwrap();
    let file_before = fixture.text(CODEX);
    let edits = [PersonalRuleEdit {
        layer: PersonalRuleLayer::Tool {
            tool: "codexcli".into(),
        },
        text: before.master.overlays["codexcli"].replacen(line, "", 1),
    }];
    let error = fixture
        .setup
        .prepare_rule_plan(&before, &edits, &[], None, &fixture_rule_renderer)
        .err()
        .expect("a duplicate donor cannot be identified safely");
    assert!(error.to_string().contains("unclear origin"), "{error:#}");
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.authored_rules, before.authored_rules);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(fixture.text(CODEX), file_before);
}

#[test]
fn setup_readoption_does_not_duplicate_a_restored_donor_addition() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
        .unwrap()
        .to_owned();
    let addition = "A restored local rule.\n";
    accept_local_rule_additions(&fixture, addition);
    let first = fixture.setup.restore().unwrap();
    assert!(first.blocked.is_empty(), "{first:?}");
    fixture.adopt_from_plan();
    let second = fixture.setup.restore().unwrap();
    assert!(second.blocked.is_empty(), "{second:?}");
    assert_eq!(fixture.text(CODEX), format!("{original}{addition}"));
}

#[test]
fn personal_rule_plan_removes_a_previously_shared_donor_addition() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
        .unwrap()
        .to_owned();
    let addition = "A portable donor rule.\n";
    accept_local_rule_additions(&fixture, addition);
    let off = fixture.setup.preview_source_off().unwrap();
    fixture
        .setup
        .turn_source_off(&SharedSourceOffRequest {
            plan_id: off.plan_id,
            token: off.token,
        })
        .unwrap();
    let preview = fixture
        .setup
        .build_share_preview(Some(&[]), false, fixture_skill_renderer)
        .unwrap();
    fixture
        .setup
        .apply_share(
            &preview.token,
            super::super::share::KeyAnswer::No,
            Some(&[]),
            &[],
            true,
            (fixture_rule_renderer, fixture_skill_renderer),
        )
        .unwrap();
    let master = fixture.setup.load().unwrap().unwrap().master;
    assert!(master.shared.contains(addition));
    let plan = propose(
        &fixture,
        vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: master.shared.replace(addition, ""),
        }],
    );
    apply(&fixture, &plan).unwrap();
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(fixture.text(CODEX), original);
}

fn share_fixture_rules(fixture: &Fixture) {
    let preview = fixture
        .setup
        .build_share_preview(Some(&[]), false, fixture_skill_renderer)
        .unwrap();
    fixture
        .setup
        .apply_share(
            &preview.token,
            super::super::share::KeyAnswer::No,
            Some(&[]),
            &[],
            true,
            (fixture_rule_renderer, fixture_skill_renderer),
        )
        .unwrap();
}

#[test]
fn personal_rule_plan_keeps_a_shared_donor_moved_back_to_its_overlay() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    for source_off in [false, true] {
        let fixture = adopted_fixture();
        let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
            .unwrap()
            .to_owned();
        let addition = "A portable donor rule.\n";
        accept_local_rule_additions(&fixture, addition);
        if source_off {
            let off = fixture.setup.preview_source_off().unwrap();
            fixture
                .setup
                .turn_source_off(&SharedSourceOffRequest {
                    plan_id: off.plan_id,
                    token: off.token,
                })
                .unwrap();
        }
        share_fixture_rules(&fixture);
        let master = fixture.setup.load().unwrap().unwrap().master;
        assert!(master.shared.contains(addition));
        let plan = propose(
            &fixture,
            vec![
                PersonalRuleEdit {
                    layer: PersonalRuleLayer::Shared,
                    text: master.shared.replace(addition, ""),
                },
                PersonalRuleEdit {
                    layer: PersonalRuleLayer::Tool {
                        tool: "codexcli".into(),
                    },
                    text: format!("{}{addition}", master.overlays["codexcli"]),
                },
            ],
        );
        apply(&fixture, &plan).unwrap();
        let restored = fixture.setup.restore().unwrap();
        assert!(
            restored.blocked.is_empty(),
            "source_off={source_off}: {restored:?}"
        );
        assert_eq!(fixture.text(CODEX), format!("{original}{addition}"));
    }
}

#[test]
fn followed_source_deletion_does_not_resurrect_a_shared_donor() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
        .unwrap()
        .to_owned();
    let addition = "A portable donor rule.\n";
    accept_local_rule_additions(&fixture, addition);
    share_fixture_rules(&fixture);
    let source = fixture.text(SHARED_RULES);
    assert!(source.contains(addition));
    fixture.write(
        SHARED_RULES,
        &format!("{}A different shared rule.\n", source.replace(addition, "")),
    );
    let synced = fixture.sync_with("");
    assert!(synced.blocked.is_empty(), "{synced:?}");
    let off = fixture.setup.preview_source_off().unwrap();
    fixture
        .setup
        .turn_source_off(&SharedSourceOffRequest {
            plan_id: off.plan_id,
            token: off.token,
        })
        .unwrap();
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(fixture.text(CODEX), original);
}

#[test]
fn personal_rule_plan_can_delete_all_identical_donor_occurrences() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
        .unwrap()
        .to_owned();
    let line = "A repeated local rule.\n";
    accept_local_rule_additions(&fixture, &format!("{line}{line}"));
    let master = fixture.setup.load().unwrap().unwrap().master;
    let plan = propose(
        &fixture,
        vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: "codexcli".into(),
            },
            text: master.overlays["codexcli"].replace(line, ""),
        }],
    );
    apply(&fixture, &plan).unwrap();
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(fixture.text(CODEX), original);
}

#[test]
fn personal_rule_plan_refuses_an_ambiguous_move_to_another_tool() {
    use super::super::tests::accept_local_rule_additions;

    let fixture = adopted_fixture();
    let line = "A local donor rule.\n";
    accept_local_rule_additions(&fixture, line);
    let before = fixture.setup.load().unwrap().unwrap();
    let edits = [
        PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: "codexcli".into(),
            },
            text: before.master.overlays["codexcli"].replace(line, ""),
        },
        PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: "claudecode".into(),
            },
            text: format!("{}{line}", before.master.overlays["claudecode"]),
        },
    ];
    let error = fixture
        .setup
        .prepare_rule_plan(&before, &edits, &[], None, &fixture_rule_renderer)
        .err()
        .expect("a cross-tool donor move needs an unambiguous origin");
    assert!(error.to_string().contains("origin"), "{error:#}");
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.authored_rules, before.authored_rules);
    assert_eq!(after.master.overlays, before.master.overlays);
}

#[test]
fn shared_donor_removal_refuses_an_indistinguishable_existing_overlay_copy() {
    use super::super::tests::accept_local_rule_additions;

    for source_off in [false, true] {
        let fixture = adopted_fixture();
        let line = "A portable donor rule.\n";
        accept_local_rule_additions(&fixture, line);
        if source_off {
            let off = fixture.setup.preview_source_off().unwrap();
            fixture
                .setup
                .turn_source_off(&SharedSourceOffRequest {
                    plan_id: off.plan_id,
                    token: off.token,
                })
                .unwrap();
        }
        share_fixture_rules(&fixture);
        let master = fixture.setup.load().unwrap().unwrap().master;
        let add_copy = propose(
            &fixture,
            vec![PersonalRuleEdit {
                layer: PersonalRuleLayer::Tool {
                    tool: "codexcli".into(),
                },
                text: format!("{}{line}", master.overlays["codexcli"]),
            }],
        );
        apply(&fixture, &add_copy).unwrap();
        let before = fixture.setup.load().unwrap().unwrap();
        let edit = [PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: before.master.shared.replace(line, ""),
        }];
        let error = fixture
            .setup
            .prepare_rule_plan(&before, &edit, &[], None, &fixture_rule_renderer)
            .err()
            .expect("the surviving duplicate has an ambiguous origin");
        assert!(error.to_string().contains("origin"), "{error:#}");
        assert_eq!(
            fixture.setup.load().unwrap().unwrap().authored_rules,
            before.authored_rules
        );
    }
}

#[test]
fn followed_source_removal_refuses_an_indistinguishable_overlay_copy() {
    use super::super::tests::accept_local_rule_additions;

    let fixture = adopted_fixture();
    let line = "A portable donor rule.\n";
    accept_local_rule_additions(&fixture, line);
    share_fixture_rules(&fixture);
    let master = fixture.setup.load().unwrap().unwrap().master;
    let add_copy = propose(
        &fixture,
        vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: "codexcli".into(),
            },
            text: format!("{}{line}", master.overlays["codexcli"]),
        }],
    );
    apply(&fixture, &add_copy).unwrap();
    let source = fixture.text(SHARED_RULES);
    fixture.write(SHARED_RULES, &source.replace(line, ""));
    let _lock = fixture.setup.lock().unwrap();
    let mut manifest = fixture.setup.load().unwrap().unwrap();
    let before = manifest.authored_rules.clone();
    let error = fixture
        .setup
        .follow_shared_source(&mut manifest)
        .unwrap_err();
    assert!(error.to_string().contains("origin"), "{error:#}");
    assert_eq!(manifest.authored_rules, before);
}

#[test]
fn legacy_donor_receipts_are_recovered_or_refused_without_losing_local_lines() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    for recoverable in [true, false] {
        let fixture = adopted_fixture();
        let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
            .unwrap()
            .to_owned();
        let shared = "A portable donor rule.\n";
        let local = "Use Codex to check this workspace.\n";
        accept_local_rule_additions(&fixture, &format!("{shared}{local}"));
        share_fixture_rules(&fixture);
        let saved = fixture.setup.load().unwrap().unwrap();
        let mut json = serde_json::to_value(&saved).unwrap();
        let authored = json["authored_rules"][CODEX].as_object_mut().unwrap();
        authored.remove("reference");
        let lines = authored["lines"].as_array_mut().unwrap();
        assert!(lines.iter().any(|line| line["shared"] == true));
        assert!(lines.iter().any(|line| line["shared"] == false));
        for line in lines {
            line.as_object_mut().unwrap().remove("position");
        }
        let mut legacy: Manifest = serde_json::from_value(json).unwrap();
        if recoverable {
            let Kind::Rule { base, .. } =
                &mut legacy.delivered.get_mut(Path::new(CODEX)).unwrap().kind
            else {
                panic!("the donor receipt must be a rule");
            };
            assert!(base.contains(local));
            *base = base.replace(local, "");
        }
        fixture.setup.save(&legacy).unwrap();
        let manifest = fixture.setup.load().unwrap().unwrap();
        let edit = [PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: format!("{}Another shared rule.\n", manifest.master.shared),
        }];
        if recoverable {
            let plan = propose(&fixture, edit.to_vec());
            apply(&fixture, &plan).unwrap();
            let restored = fixture.setup.restore().unwrap();
            assert!(restored.blocked.is_empty(), "{restored:?}");
            assert_eq!(fixture.text(CODEX), format!("{original}{shared}{local}"));
        } else {
            let before = fixture.text(CODEX);
            let error = fixture
                .setup
                .prepare_rule_plan(&manifest, &edit, &[], None, &fixture_rule_renderer)
                .err()
                .expect("an unverified legacy donor must be refused");
            assert!(error.to_string().contains("older saved rule"), "{error:#}");
            let restored = fixture.setup.restore().unwrap();
            assert!(
                restored
                    .blocked
                    .iter()
                    .any(|line| line.contains("older saved rule")),
                "{restored:?}"
            );
            assert_eq!(fixture.text(CODEX), before);
        }
    }
}

#[test]
fn stale_legacy_shared_donor_is_refused_instead_of_resurrected() {
    use super::super::tests::{accept_local_rule_additions, CODEX};

    let fixture = adopted_fixture();
    let line = "A portable donor rule.\n";
    accept_local_rule_additions(&fixture, line);
    share_fixture_rules(&fixture);
    let old_authored = fixture.setup.load().unwrap().unwrap().authored_rules;
    let source = fixture.text(SHARED_RULES);
    fixture.write(
        SHARED_RULES,
        &format!("{}Another shared rule.\n", source.replace(line, "")),
    );
    assert!(fixture.sync_with("").blocked.is_empty());
    let mut stale = fixture.setup.load().unwrap().unwrap();
    stale.authored_rules = old_authored;
    let mut json = serde_json::to_value(stale).unwrap();
    let authored = json["authored_rules"][CODEX].as_object_mut().unwrap();
    authored.remove("reference");
    for item in authored["lines"].as_array_mut().unwrap() {
        item.as_object_mut().unwrap().remove("position");
    }
    let stale: Manifest = serde_json::from_value(json).unwrap();
    fixture.setup.save(&stale).unwrap();
    let before = fixture.text(CODEX);
    let edit = [PersonalRuleEdit {
        layer: PersonalRuleLayer::Shared,
        text: format!("{}A later shared rule.\n", stale.master.shared),
    }];
    let error = fixture
        .setup
        .prepare_rule_plan(&stale, &edit, &[], None, &fixture_rule_renderer)
        .err()
        .expect("a stale donor cannot be safely restored");
    assert!(error.to_string().contains("origin"), "{error:#}");
    let restored = fixture.setup.restore().unwrap();
    assert!(
        restored.blocked.iter().any(|item| item.contains("origin")),
        "{restored:?}"
    );
    assert_eq!(fixture.text(CODEX), before);
    assert!(!before.contains(line));
}

#[test]
fn setup_restore_records_a_recovered_legacy_override_with_unknown_origin() {
    let fixture = adopted_fixture();
    let mut manifest = fixture.setup.load().unwrap().unwrap();
    let tool = "codexcli";
    let base = manifest.master.base_rule_text(tool, &[]);
    manifest.master.overrides.insert(
        tool.into(),
        LocalText {
            edited: format!("{base}A legacy accepted addition.\n"),
            base,
        },
    );
    fixture.setup.save(&manifest).unwrap();
    let synced = fixture.sync_with("");
    assert!(synced.blocked.is_empty(), "{synced:?}");
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    let saved = fixture.setup.load().unwrap().unwrap();
    assert!(saved.master.overrides.is_empty());
    assert!(saved.master.overlays[tool].contains("A legacy accepted addition."));
    let version = read_version(
        &fixture.setup,
        &saved,
        saved.personal_rule_history.last().unwrap(),
    )
    .unwrap();
    assert_eq!(version.summary.author, None);
    assert_eq!(version.summary.confirmer, None);
    assert!(version.rules.iter().any(|rule| {
        matches!(&rule.layer, PersonalRuleLayer::Tool { tool: id } if id == tool)
            && rule.text.contains("A legacy accepted addition.")
    }));
}

#[test]
fn retired_source_hardlink_is_not_read_by_rule_routes() {
    let fixture = adopted_fixture();
    let preview = fixture.setup.preview_source_off().unwrap();
    fixture
        .setup
        .turn_source_off(&SharedSourceOffRequest {
            plan_id: preview.plan_id,
            token: preview.token,
        })
        .unwrap();
    let source = fixture.setup.home.join(SHARED_RULES);
    let tool = fixture.setup.home.join(super::super::tests::CLAUDE);
    let marker = "synthetic retired source marker";
    std::fs::write(&source, marker).unwrap();
    std::fs::remove_file(&tool).unwrap();
    std::fs::hard_link(&source, &tool).unwrap();

    let root = fixture.setup.home_files().unwrap();
    let error = root
        .read(Path::new(super::super::tests::CLAUDE), false)
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert!(root.with_shared_rule_source().is_err());

    let status = fixture.setup.status();
    assert!(!format!("{status:?}").contains(marker));
    let share = fixture.setup.preview_share_using(fixture_skill_renderer);
    assert!(!format!("{share:?}").contains(marker));
    let adoption = fixture
        .setup
        .preview_adoption_using(fixture_rule_renderer, fixture_skill_renderer);
    assert!(!format!("{adoption:?}").contains(marker));
    let restore = fixture.setup.restore();
    assert!(!format!("{restore:?}").contains(marker));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), marker);
}

#[test]
fn retired_source_hardlink_in_private_data_is_not_read() {
    let fixture = adopted_fixture();
    let preview = fixture.setup.preview_source_off().unwrap();
    fixture
        .setup
        .turn_source_off(&SharedSourceOffRequest {
            plan_id: preview.plan_id,
            token: preview.token,
        })
        .unwrap();
    let source = fixture.setup.home.join(SHARED_RULES);
    let marker = "synthetic private-data alias marker";
    std::fs::write(&source, marker).unwrap();
    let manifest = fixture.setup.data.join(storage::MANIFEST);
    std::fs::remove_file(&manifest).unwrap();
    std::fs::hard_link(&source, &manifest).unwrap();

    let status = fixture.setup.status();
    assert!(status.is_err());
    assert!(!format!("{status:?}").contains(marker));
    assert_eq!(std::fs::read_to_string(&source).unwrap(), marker);
}

#[test]
fn settings_only_source_off_adoption_does_not_read_the_source() {
    let fixture = Fixture::new();
    let mut manifest = empty_manifest();
    manifest.settings_only = true;
    {
        let _lock = fixture.setup.lock().unwrap();
        fixture.setup.save(&manifest).unwrap();
    }
    fixture.write(super::super::tests::CLAUDE, "Claude-only rule.\n");
    fixture.write(super::super::tests::CODEX, "Codex-only rule.\n");
    let source = fixture.setup.home.join(SHARED_RULES);
    std::fs::remove_file(&source).unwrap();
    std::fs::create_dir(&source).unwrap();

    let plan = fixture
        .setup
        .preview_adoption_using(fixture_rule_renderer, fixture_skill_renderer)
        .unwrap();
    fixture
        .setup
        .adopt_using(&plan.token, fixture_rule_renderer, fixture_skill_renderer)
        .unwrap();
    assert!(
        !fixture
            .setup
            .load()
            .unwrap()
            .unwrap()
            .shared_source_following
    );
    assert!(source.is_dir());
}

fn shared_and_claude_edits(fixture: &Fixture) -> (Vec<PersonalRuleEdit>, String, String) {
    let manifest = fixture.setup.load().unwrap().unwrap();
    let shared_before = manifest.master.shared.clone();
    let overlay_before = manifest
        .master
        .overlays
        .get(CLAUDE_RULES)
        .expect("the synthetic Claude fixture has tool-only rules")
        .clone();
    let shared_after = format!("{shared_before}Added shared rule.\n");
    let overlay_after = format!("{overlay_before}Added Claude-only rule.\n");
    (
        vec![
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Shared,
                text: shared_after,
            },
            PersonalRuleEdit {
                layer: PersonalRuleLayer::Tool {
                    tool: CLAUDE_RULES.into(),
                },
                text: overlay_after,
            },
        ],
        shared_before,
        overlay_before,
    )
}

fn propose(fixture: &Fixture, edits: Vec<PersonalRuleEdit>) -> PersonalRulePlan {
    propose_with_origin(fixture, edits, Some("claude-code".into()), None)
}

fn propose_with_origin(
    fixture: &Fixture,
    edits: Vec<PersonalRuleEdit>,
    proposer: Option<String>,
    restore: Option<PersonalRuleVersion>,
) -> PersonalRulePlan {
    let _lock = fixture.setup.lock().unwrap();
    let manifest = fixture
        .setup
        .load_if_present()
        .unwrap()
        .unwrap_or_else(empty_manifest);
    let edits = canonical_edits(edits).unwrap();
    let prepared = fixture
        .setup
        .prepare_rule_plan(&manifest, &edits, &[], proposer, &fixture_rule_renderer)
        .unwrap();
    remember_plan(&fixture.setup, prepared, edits, restore)
}

fn apply(fixture: &Fixture, plan: &PersonalRulePlan) -> Result<PersonalRuleVersion> {
    fixture
        .setup
        .apply_rule_plan(&plan.id, &[], &fixture_rule_renderer)
}

#[test]
fn ux_skill_28_plan_applies_shared_and_overlay_with_origins() {
    let fixture = adopted_fixture();
    let (edits, shared_before, overlay_before) = shared_and_claude_edits(&fixture);
    let shared_after = edits[0].text.clone();
    let overlay_after = edits[1].text.clone();
    let plan = propose(&fixture, edits);

    assert_eq!(plan.changes.len(), 2);
    assert!(plan.blocked_tools.is_empty(), "{plan:?}");
    assert!(matches!(&plan.changes[0].layer, PersonalRuleLayer::Shared));
    assert!(matches!(
        &plan.changes[1].layer,
        PersonalRuleLayer::Tool { tool } if tool == CLAUDE_RULES
    ));

    let applied = apply(&fixture, &plan).unwrap();
    let manifest = fixture.setup.load().unwrap().unwrap();
    assert_eq!(manifest.master.shared, shared_after);
    assert_eq!(manifest.master.overlays[CLAUDE_RULES], overlay_after);
    assert_eq!(manifest.personal_rule_history.len(), 2);
    assert_eq!(applied.summary.cause, "change");
    assert_eq!(applied.summary.author.as_deref(), Some("person"));
    assert_eq!(applied.summary.proposer.as_deref(), Some("claude-code"));
    assert_eq!(applied.summary.confirmer.as_deref(), Some("person"));
    assert_eq!(
        manifest.personal_rule_origins.len(),
        2,
        "one origin record is stored for each changed layer"
    );
    for layer in [
        PersonalRuleLayer::Shared,
        PersonalRuleLayer::Tool {
            tool: CLAUDE_RULES.into(),
        },
    ] {
        let record = manifest
            .personal_rule_origins
            .iter()
            .find(|record| record.layer == layer)
            .expect("changed layer has an origin record");
        let origin = record.lines.last().and_then(Option::as_ref).unwrap();
        assert_eq!(origin.author.as_deref(), Some("person"));
        assert_eq!(origin.proposer.as_deref(), Some("claude-code"));
        assert_eq!(origin.confirmer.as_deref(), Some("person"));
    }

    let history = manifest.personal_rule_history.clone();
    let baseline = read_version(&fixture.setup, &manifest, &history[0]).unwrap();
    let change = read_version(&fixture.setup, &manifest, &history[1]).unwrap();
    assert_eq!(baseline.summary.cause, "baseline");
    assert_eq!(change.summary.id, applied.summary.id);
    assert_eq!(change.summary.layers.len(), 2);
    assert!(fixture
        .text(super::super::tests::CLAUDE)
        .contains("Added shared rule."));
    assert!(fixture
        .text(super::super::tests::CLAUDE)
        .contains("Added Claude-only rule."));
    assert!(fixture
        .text(super::super::tests::CODEX)
        .contains("Added shared rule."));
    assert!(!fixture
        .text(super::super::tests::CODEX)
        .contains("Added Claude-only rule."));

    // The first version preserves the initial content for a later restore.
    assert!(baseline
        .rules
        .iter()
        .any(|rule| { rule.layer == PersonalRuleLayer::Shared && rule.text == shared_before }));
    assert!(baseline.rules.iter().any(|rule| {
        rule.layer
            == (PersonalRuleLayer::Tool {
                tool: CLAUDE_RULES.into(),
            })
            && rule.text == overlay_before
    }));
}

#[test]
fn personal_rule_plan_refuses_stale_tool_projection_without_mutating_rules_or_history() {
    let fixture = adopted_fixture();
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let local_edit = "person edited this managed file after the preview\n";
    fixture.write(super::super::tests::CLAUDE, local_edit);

    let error = apply(&fixture, &plan).unwrap_err().to_string();
    assert!(error.contains("changed after the preview"), "{error}");
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(after.personal_rule_origins, before.personal_rule_origins);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
    assert_eq!(fixture.text(super::super::tests::CLAUDE), local_edit);
}

#[test]
fn personal_rule_plan_refuses_stale_source_without_mutating_rules_or_history() {
    let fixture = adopted_fixture();
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let source_edit = format!("{}Concurrent source line.\n", before.master.shared);
    fixture.write(super::super::SHARED_RULES, &source_edit);

    let error = apply(&fixture, &plan).unwrap_err().to_string();
    assert!(
        error.contains("Shared source changes are waiting"),
        "{error}"
    );
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(after.personal_rule_origins, before.personal_rule_origins);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
    assert_eq!(fixture.text(super::super::SHARED_RULES), source_edit);
}

#[test]
fn personal_rule_plan_rolls_back_if_source_changes_during_delivery() {
    let fixture = adopted_fixture();
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let claude_before = fixture.text(super::super::tests::CLAUDE);
    let codex_before = fixture.text(super::super::tests::CODEX);
    let source_path = fixture.setup.home.join(SHARED_RULES);
    let source_after = format!("{}A concurrent source edit.\n", fixture.text(SHARED_RULES));
    let changed = source_after.clone();
    super::super::share_rules::at_rule_share_write(super::super::tests::CODEX, move || {
        std::fs::write(source_path, changed).unwrap();
    });

    let error = format!("{:#}", apply(&fixture, &plan).unwrap_err());
    assert!(
        error.contains("source") && error.contains("changed"),
        "{error}"
    );
    assert_eq!(fixture.text(SHARED_RULES), source_after);
    assert_eq!(fixture.text(super::super::tests::CLAUDE), claude_before);
    assert_eq!(fixture.text(super::super::tests::CODEX), codex_before);
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
    assert!(after.personal_rule_journal.is_none());
    assert!(after.pending.is_empty());
}

#[test]
fn followed_source_sync_records_one_change_with_unknown_author_and_confirmer() {
    let fixture = adopted_fixture();
    let source_before = fixture.text(super::super::SHARED_RULES);
    let source_after = format!("{source_before}Automatic source addition.\n");
    fixture.write(super::super::SHARED_RULES, &source_after);

    let report = fixture.sync_generated(&[]);
    assert!(report.blocked.is_empty(), "{report:?}");
    let manifest = fixture.setup.load().unwrap().unwrap();
    assert!(manifest.shared_source_following);
    assert_eq!(manifest.master.shared, source_after);
    assert_eq!(manifest.personal_rule_history.len(), 2);
    let baseline = read_version(
        &fixture.setup,
        &manifest,
        &manifest.personal_rule_history[0],
    )
    .unwrap();
    let change = read_version(
        &fixture.setup,
        &manifest,
        &manifest.personal_rule_history[1],
    )
    .unwrap();
    assert_eq!(baseline.summary.cause, "baseline");
    assert_eq!(change.summary.cause, "change");
    assert_eq!(change.summary.layers, vec![PersonalRuleLayer::Shared]);
    assert_eq!(change.summary.author, None);
    assert_eq!(change.summary.proposer, None);
    assert_eq!(change.summary.confirmer, None);
    let origin = manifest
        .personal_rule_origins
        .iter()
        .find(|record| record.layer == PersonalRuleLayer::Shared)
        .unwrap()
        .lines
        .last()
        .and_then(Option::as_ref)
        .unwrap();
    assert_eq!(origin.author, None);
    assert_eq!(origin.proposer, None);
    assert_eq!(origin.confirmer, None);
}

#[test]
fn person_confirmed_rule_share_records_one_change_with_person_confirmer() {
    let fixture = Fixture::new();
    let source = fixture.text(super::super::SHARED_RULES);
    fixture.write(
        super::super::tests::CODEX,
        &format!("{source}- Check the final exit status.\n"),
    );
    fixture.adopt_from_plan();
    let plan = fixture
        .setup
        .build_share_preview(Some(&[]), false, fixture_skill_renderer)
        .unwrap();
    assert!(plan.rules.iter().any(|rule| !rule.kept), "{plan:?}");

    let report = fixture
        .setup
        .apply_share(
            &plan.token,
            super::super::share::KeyAnswer::No,
            Some(&[]),
            &[],
            true,
            (fixture_rule_renderer, fixture_skill_renderer),
        )
        .unwrap();
    assert!(report.blocked.is_empty(), "{report:?}");
    let manifest = fixture.setup.load().unwrap().unwrap();
    assert_eq!(manifest.personal_rule_history.len(), 2);
    let baseline = read_version(
        &fixture.setup,
        &manifest,
        &manifest.personal_rule_history[0],
    )
    .unwrap();
    let change = read_version(
        &fixture.setup,
        &manifest,
        &manifest.personal_rule_history[1],
    )
    .unwrap();
    assert_eq!(baseline.summary.cause, "baseline");
    assert_eq!(change.summary.cause, "change");
    assert!(!change.summary.layers.is_empty());
    assert_eq!(change.summary.author.as_deref(), Some("person"));
    assert_eq!(change.summary.proposer, None);
    assert_eq!(change.summary.confirmer.as_deref(), Some("person"));
    assert!(manifest.personal_rule_origins.iter().any(|record| {
        record.layer == PersonalRuleLayer::Shared
            && record.lines.iter().flatten().any(|origin| {
                origin.author.as_deref() == Some("person")
                    && origin.confirmer.as_deref() == Some("person")
            })
    }));
}

#[test]
fn personal_rule_plan_credential_refusal_does_not_echo_the_secret() {
    let fixture = adopted_fixture();
    let manifest = fixture.setup.load().unwrap().unwrap();
    let secret = "sk-syntheticfixturecredentialvalue123456";
    let edits = canonical_edits(vec![PersonalRuleEdit {
        layer: PersonalRuleLayer::Shared,
        text: format!("{}Credential: {secret}\n", manifest.master.shared),
    }])
    .unwrap();
    let result = {
        let _lock = fixture.setup.lock().unwrap();
        fixture.setup.prepare_rule_plan(
            &manifest,
            &edits,
            &[],
            Some("claude-code".into()),
            &fixture_rule_renderer,
        )
    };
    let error = match result {
        Err(error) => server_error(error).to_string(),
        Ok(_) => panic!("credential-bearing personal rule text was accepted"),
    };
    assert!(error.contains("credential"), "{error}");
    assert!(error.contains("layer=shared line="), "{error}");
    assert!(
        !error.contains(secret),
        "credential text was echoed: {error}"
    );

    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, manifest.master.shared);
    assert_eq!(after.master.overlays, manifest.master.overlays);
    assert!(after.personal_rule_history.is_empty());
}

#[test]
fn personal_rule_plan_is_one_use_and_does_not_add_history_on_replay() {
    let fixture = adopted_fixture();
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    apply(&fixture, &plan).unwrap();
    let after_first_apply = fixture.setup.load().unwrap().unwrap();

    let error = apply(&fixture, &plan).unwrap_err().to_string();
    assert!(error.contains("expired"), "{error}");
    let after_replay = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after_replay.master.shared, after_first_apply.master.shared);
    assert_eq!(
        after_replay.master.overlays,
        after_first_apply.master.overlays
    );
    assert_eq!(
        after_replay.personal_rule_history,
        after_first_apply.personal_rule_history
    );
    assert_eq!(after_replay.personal_rule_history.len(), 2);
}

#[test]
fn personal_rule_plan_expiry_requires_a_fresh_preview() {
    let fixture = adopted_fixture();
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let id = Uuid::parse_str(&plan.id).unwrap();
    PLANS.lock().unwrap().get_mut(&id).unwrap().created =
        Instant::now() - PLAN_LIFETIME - Duration::from_secs(1);
    let error = apply(&fixture, &plan).unwrap_err().to_string();
    assert!(error.contains("expired"), "{error}");
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
}

fn check_personal_rule_batch_rollback(source_following: bool) {
    let fixture = adopted_fixture();
    if !source_following {
        let preview = fixture.setup.preview_source_off().unwrap();
        fixture
            .setup
            .turn_source_off(&SharedSourceOffRequest {
                plan_id: preview.plan_id,
                token: preview.token,
            })
            .unwrap();
    }
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let claude_before = fixture.text(super::super::tests::CLAUDE);
    let history_dir = fixture.setup.data.join(HISTORY_DIRECTORY);
    let history_files = || -> BTreeSet<_> {
        std::fs::read_dir(&history_dir)
            .into_iter()
            .flatten()
            .map(|entry| entry.unwrap().file_name())
            .collect()
    };
    let history_before = history_files();
    let concurrent = "Concurrent edit to the second tool file.\n".to_owned();
    let expected_concurrent = concurrent.clone();
    let codex_path = fixture.setup.home.join(super::super::tests::CODEX);
    let claude_path = fixture.setup.home.join(super::super::tests::CLAUDE);
    super::super::share_rules::at_rule_share_write(super::super::tests::CODEX, move || {
        let earlier = std::fs::read_to_string(claude_path).unwrap();
        assert!(earlier.contains("Added shared rule."));
        assert!(earlier.contains("Added Claude-only rule."));
        std::fs::write(codex_path, concurrent).unwrap();
    });

    let error = apply(&fixture, &plan).unwrap_err().to_string();
    assert!(error.contains("tool file changed"), "{error}");
    assert_eq!(fixture.text(super::super::tests::CLAUDE), claude_before);
    assert_eq!(
        fixture.text(super::super::tests::CODEX),
        expected_concurrent
    );

    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(after.personal_rule_origins, before.personal_rule_origins);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
    assert!(after.personal_rule_journal.is_none());
    assert!(after.pending.is_empty());
    assert!(after.stages.is_empty());
    assert_eq!(
        history_files(),
        history_before,
        "failed batch should remove its uncommitted history files"
    );
}

#[test]
fn personal_rule_batch_rolls_back_earlier_tool_and_preserves_concurrent_second_write() {
    check_personal_rule_batch_rollback(true);
}

#[test]
fn personal_rule_batch_rolls_back_earlier_tool_when_source_is_off() {
    check_personal_rule_batch_rollback(false);
}

fn check_personal_rule_interrupted_recovery(source_following: bool) {
    let fixture = adopted_fixture();
    super::super::tests::accept_local_rule_additions(&fixture, "An approved donor rule.\n");
    if !source_following {
        let preview = fixture.setup.preview_source_off().unwrap();
        fixture
            .setup
            .turn_source_off(&SharedSourceOffRequest {
                plan_id: preview.plan_id,
                token: preview.token,
            })
            .unwrap();
    }
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let claude_before = fixture.text(super::super::tests::CLAUDE);
    let codex_before = fixture.text(super::super::tests::CODEX);
    super::super::share_rules::at_rule_share_write(super::super::tests::CODEX, || {
        panic!("synthetic interruption after the first tool write");
    });
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = apply(&fixture, &plan);
    }));
    assert!(interrupted.is_err());
    let _lock = fixture.setup.lock().unwrap();
    let mut pending = fixture.setup.load().unwrap().unwrap();
    assert!(pending.rule_share.is_some());
    assert!(pending.personal_rule_journal.is_some());
    let root = fixture.setup.home_files().unwrap();
    fixture.setup.recover_pending(&root, &mut pending).unwrap();
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(after.authored_rules, before.authored_rules);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
    assert!(after.personal_rule_journal.is_none());
    assert_eq!(fixture.text(super::super::tests::CLAUDE), claude_before);
    assert_eq!(fixture.text(super::super::tests::CODEX), codex_before);
}

#[test]
fn personal_rule_recovery_rolls_back_an_interrupted_multi_tool_write() {
    check_personal_rule_interrupted_recovery(true);
}

#[test]
fn personal_rule_recovery_rolls_back_an_interrupted_write_when_source_is_off() {
    check_personal_rule_interrupted_recovery(false);
}

#[test]
fn personal_rule_final_checkpoint_failure_recovers_the_durable_journal() {
    let fixture = adopted_fixture();
    super::super::tests::accept_local_rule_additions(&fixture, "An approved donor rule.\n");
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let claude = fixture.text(super::super::tests::CLAUDE);
    let codex = fixture.text(super::super::tests::CODEX);
    super::super::share_rules::fail_next_rule_final_checkpoint();
    let error = apply(&fixture, &plan).unwrap_err();
    assert!(error.to_string().contains("not applied"), "{error:#}");
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(after.master.shared, before.master.shared);
    assert_eq!(after.master.overlays, before.master.overlays);
    assert_eq!(after.authored_rules, before.authored_rules);
    assert_eq!(after.personal_rule_history, before.personal_rule_history);
    assert!(after.rule_share.is_none());
    assert!(after.personal_rule_journal.is_none());
    assert!(after.pending.is_empty());
    assert_eq!(fixture.text(super::super::tests::CLAUDE), claude);
    assert_eq!(fixture.text(super::super::tests::CODEX), codex);
}

#[test]
fn personal_rule_post_checkpoint_error_reports_an_uncertain_committed_change() {
    let fixture = adopted_fixture();
    let addition = "An approved donor rule.\n";
    super::super::tests::accept_local_rule_additions(&fixture, addition);
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let plan = propose(&fixture, edits);
    let before = fixture.setup.load().unwrap().unwrap();
    let original = text(&before.backup[Path::new(super::super::tests::CODEX)])
        .unwrap()
        .to_owned();
    super::super::share_rules::fail_after_next_rule_final_checkpoint();
    let error = apply(&fixture, &plan).unwrap_err();
    assert!(
        error.to_string().contains("may have been applied"),
        "{error:#}"
    );
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(
        after.personal_rule_history.len(),
        before.personal_rule_history.len() + 1
    );
    assert_ne!(after.master.shared, before.master.shared);
    assert!(after.rule_share.is_none());
    assert!(after.personal_rule_journal.is_none());
    assert!(after.pending.is_empty());
    let restored = fixture.setup.restore().unwrap();
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(
        fixture.text(super::super::tests::CODEX),
        format!("{original}{addition}")
    );
}

#[test]
fn unmanaged_personal_rule_checkpoint_failures_preserve_a_clear_outcome() {
    for after_save in [false, true] {
        let fixture = adopted_fixture();
        let before = fixture.setup.load().unwrap().unwrap();
        let plan = propose(
            &fixture,
            vec![PersonalRuleEdit {
                layer: PersonalRuleLayer::Tool {
                    tool: "geminicli".into(),
                },
                text: "A saved but unmanaged tool rule.\n".into(),
            }],
        );
        assert!(plan.unmanaged_tools.contains(&"geminicli".to_owned()));
        if after_save {
            super::super::share_rules::fail_after_next_rule_final_checkpoint();
        } else {
            super::super::share_rules::fail_next_rule_final_checkpoint();
        }
        let error = apply(&fixture, &plan).unwrap_err();
        let after = fixture.setup.load().unwrap().unwrap();
        assert!(after.personal_rule_journal.is_none());
        assert!(after.pending.is_empty());
        if after_save {
            assert!(
                error.to_string().contains("may have been applied"),
                "{error:#}"
            );
            assert_eq!(
                after.master.overlays["geminicli"],
                "A saved but unmanaged tool rule.\n"
            );
            assert_eq!(
                after.personal_rule_history.len(),
                before.personal_rule_history.len() + 2
            );
        } else {
            assert!(error.to_string().contains("not applied"), "{error:#}");
            assert_eq!(after.master.overlays, before.master.overlays);
            assert_eq!(after.personal_rule_history, before.personal_rule_history);
        }
    }
}

#[test]
fn followed_source_checkpoint_failures_preserve_a_clear_outcome() {
    for after_save in [false, true] {
        let fixture = adopted_fixture();
        let before = fixture.setup.load().unwrap().unwrap();
        let source = format!("{}A followed source edit.\n", fixture.text(SHARED_RULES));
        fixture.write(SHARED_RULES, &source);
        if after_save {
            super::super::share_rules::fail_after_next_rule_final_checkpoint();
        } else {
            super::super::share_rules::fail_next_rule_final_checkpoint();
        }
        let _lock = fixture.setup.lock().unwrap();
        let mut manifest = fixture.setup.load().unwrap().unwrap();
        let error = fixture
            .setup
            .follow_shared_source(&mut manifest)
            .unwrap_err();
        let after = fixture.setup.load().unwrap().unwrap();
        assert!(after.personal_rule_journal.is_none());
        if after_save {
            assert!(
                error.to_string().contains("may have been applied"),
                "{error:#}"
            );
            assert!(after.master.shared.contains("A followed source edit."));
        } else {
            assert!(error.to_string().contains("not applied"), "{error:#}");
            assert_eq!(after.master.shared, before.master.shared);
            assert_eq!(after.personal_rule_history, before.personal_rule_history);
        }
        assert_eq!(fixture.text(SHARED_RULES), source);
    }
}

#[test]
fn personal_rule_history_restore_applies_as_one_new_history_entry() {
    let fixture = adopted_fixture();
    let (edits, _, _) = shared_and_claude_edits(&fixture);
    let first_plan = propose(&fixture, edits);
    apply(&fixture, &first_plan).unwrap();
    let changed = fixture.setup.load().unwrap().unwrap();
    let history = changed.personal_rule_history.clone();
    let baseline = read_version(&fixture.setup, &changed, &history[0]).unwrap();
    let restore_plan = propose_with_origin(
        &fixture,
        restore_edits(&changed, &baseline).unwrap(),
        None,
        Some(baseline.clone()),
    );

    assert_eq!(restore_plan.changes.len(), 2);
    let restored = apply(&fixture, &restore_plan).unwrap();
    let after = fixture.setup.load().unwrap().unwrap();
    assert_eq!(restored.summary.cause, "restore");
    assert_eq!(after.personal_rule_history.len(), history.len() + 1);
    assert_eq!(
        after.master.shared,
        baseline
            .rules
            .iter()
            .find(|rule| rule.layer == PersonalRuleLayer::Shared)
            .unwrap()
            .text
    );
    assert_eq!(
        after.master.overlays[CLAUDE_RULES],
        baseline
            .rules
            .iter()
            .find(|rule| {
                rule.layer
                    == (PersonalRuleLayer::Tool {
                        tool: CLAUDE_RULES.into(),
                    })
            })
            .unwrap()
            .text
    );
    let new_entry = read_version(
        &fixture.setup,
        &after,
        after.personal_rule_history.last().unwrap(),
    )
    .unwrap();
    assert_eq!(new_entry.summary.id, restored.summary.id);
    assert_eq!(new_entry.summary.cause, "restore");
}

#[test]
fn personal_master_survives_setup_restore_and_reenrolls_a_tool_from_its_saved_overlay() {
    let fixture = Fixture::new();
    fixture.adopt_from_plan();

    let off = fixture.setup.preview_source_off().unwrap();
    fixture
        .setup
        .turn_source_off(&SharedSourceOffRequest {
            plan_id: off.plan_id,
            token: off.token,
        })
        .unwrap();
    fixture.setup.restore().unwrap();
    std::fs::remove_dir_all(fixture.setup.home.join(".claude")).unwrap();
    std::fs::remove_dir_all(fixture.setup.home.join(".codex")).unwrap();

    let shared_before = fixture.setup.load().unwrap().unwrap().master.shared;
    let shared_after = format!("{shared_before}Personal shared addition.\n");
    let gemini_overlay = "Personal Gemini-only addition.\n".to_owned();
    let edits = vec![
        PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: shared_after.clone(),
        },
        PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: "geminicli".into(),
            },
            text: gemini_overlay.clone(),
        },
    ];
    let plan = propose(&fixture, edits);
    assert!(plan.unmanaged_tools.contains(&"geminicli".to_owned()));
    apply(&fixture, &plan).unwrap();
    let personal = fixture.setup.load().unwrap().unwrap();
    assert_eq!(personal.master.shared, shared_after);
    assert_eq!(personal.master.overlays["geminicli"], gemini_overlay);
    assert!(!personal.master.enrolls_target("geminicli"));
    assert!(personal.master.unenrolled_overlays.contains("geminicli"));
    assert_eq!(personal.personal_rule_history.len(), 2);

    // Restore setup again with no enrolled tool. The personal store and its
    // baseline/change history remain the authority for a later adoption.
    fixture.setup.restore().unwrap();
    let restored = fixture.setup.load().unwrap().unwrap();
    assert_eq!(restored.master.shared, shared_after);
    assert_eq!(restored.master.overlays["geminicli"], gemini_overlay);
    assert_eq!(
        restored.personal_rule_history,
        personal.personal_rule_history
    );
    assert!(!restored.shared_source_following);
    assert!(restored.master.unenrolled_overlays.contains("geminicli"));
    let retained_change = read_version(
        &fixture.setup,
        &restored,
        restored.personal_rule_history.last().unwrap(),
    )
    .unwrap();
    assert_eq!(retained_change.summary.cause, "change");

    std::fs::create_dir_all(fixture.setup.home.join(".gemini")).unwrap();
    let gemini_file = restored.master.rule_text("geminicli", &[]).unwrap();
    fixture.write(".gemini/GEMINI.md", &gemini_file);
    let adoption = fixture
        .setup
        .preview_adoption_using(fixture_rule_renderer, fixture_skill_renderer)
        .unwrap();
    fixture
        .setup
        .adopt_using(
            &adoption.token,
            fixture_rule_renderer,
            fixture_skill_renderer,
        )
        .unwrap();

    let adopted = fixture.setup.load().unwrap().unwrap();
    assert!(adopted.adopted);
    assert_eq!(adopted.master.shared, shared_after);
    assert_eq!(adopted.master.overlays["geminicli"], gemini_overlay);
    assert!(!adopted.master.unenrolled_overlays.contains("geminicli"));
    assert!(adopted.master.enrolls_target("geminicli"));
    assert!(adopted.active_deliveries().any(|(_, delivery)| matches!(
        &delivery.kind,
        Kind::Rule { target, .. } if target == "geminicli"
    )));
    assert_eq!(
        adopted.personal_rule_history,
        personal.personal_rule_history
    );
}

#[test]
fn confirmed_personal_tool_rule_survives_setup_restore_while_source_following_is_on() {
    let fixture = adopted_fixture();
    let before = fixture.setup.load().unwrap().unwrap();
    let old_overlay = before.master.overlays[CLAUDE_RULES].clone();
    let next_overlay = format!("{old_overlay}Confirmed tool-only addition.\n");
    let plan = propose(
        &fixture,
        vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Tool {
                tool: CLAUDE_RULES.into(),
            },
            text: next_overlay.clone(),
        }],
    );
    apply(&fixture, &plan).unwrap();
    let confirmed = fixture.setup.load().unwrap().unwrap();
    assert!(confirmed.shared_source_following);
    assert_eq!(confirmed.personal_rule_history.len(), 2);

    let report = fixture.setup.restore().unwrap();
    assert!(report.blocked.is_empty(), "{report:?}");
    let restored = fixture.setup.load().unwrap().unwrap();
    assert_eq!(restored.master.overlays[CLAUDE_RULES], next_overlay);
    assert_eq!(
        restored.personal_rule_history,
        confirmed.personal_rule_history
    );
    assert!(restored.shared_source_following);
    assert!(restored.master.unenrolled_overlays.contains(CLAUDE_RULES));
    assert!(restored.personal_rule_journal.is_none());
}
