// SPDX-License-Identifier: Apache-2.0

use super::super::tests::{fixture_rule_renderer, fixture_skill_renderer, Fixture, SHARED_RULES};
use super::*;
use wenlan_types::personal_rules::{PersonalRuleEdit, PersonalRuleLayer};

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

fn page_rule(text: &str, id: &str, line: usize) -> PageRule {
    PageRule {
        text: text.into(),
        source_text: text.into(),
        location: wenlan_types::personal_rules::PageRuleLocation {
            page_id: Some(id.into()),
            line,
        },
        rule_id: format!("{id}:{line}"),
        author: None,
    }
}

fn preview_page_move(
    fixture: &Fixture,
    snapshot: PageRuleSnapshot,
    generated: &[String],
) -> PersonalRulePlan {
    let manifest = fixture
        .setup
        .load_if_present()
        .unwrap()
        .unwrap_or_else(empty_manifest);
    let pages = PagePlan::Merge(snapshot);
    let prepared = fixture
        .setup
        .prepare_page_plan(&manifest, &pages, generated, &fixture_rule_renderer)
        .unwrap();
    remember_plan(&fixture.setup, prepared, Vec::new(), None, Some(pages))
}

#[test]
fn personal_rules_page_merge_preserves_origins_and_round_trips_through_two_tool_files() {
    use super::super::tests::{CLAUDE, CODEX};

    let fixture = adopted_fixture();
    let generated = [
        "One rule.".into(),
        "Check the original receipt.".into(),
        "Keep meeting notes short.".into(),
    ];
    fixture.sync_generated(&generated);
    // The third line is a confirmed conversation preference: it is not
    // moved and keeps arriving from its memory.
    let snapshot = PageRuleSnapshot {
        rules: vec![
            page_rule(&generated[0], "rule-page", 4),
            page_rule(&generated[1], "rule-page", 7),
        ],
        revisions: BTreeMap::from([("rule-page".into(), 3)]),
        ..Default::default()
    };
    let old = std::fs::read(fixture.setup.data.join(storage::MANIFEST)).unwrap();
    let old_tool = std::fs::read(fixture.setup.home.join(CODEX)).unwrap();
    let plan = preview_page_move(&fixture, snapshot.clone(), &generated[2..]);
    assert_eq!(plan.page_merge.as_ref().unwrap().rule_count, 2);
    assert_eq!(
        std::fs::read(fixture.setup.data.join(storage::MANIFEST)).unwrap(),
        old
    );
    assert_eq!(
        std::fs::read(fixture.setup.home.join(CODEX)).unwrap(),
        old_tool
    );
    let version = fixture
        .setup
        .apply_plan(
            &plan.id,
            &generated[2..],
            Some(snapshot),
            &fixture_rule_renderer,
        )
        .unwrap();
    assert_eq!(version.summary.cause, MERGE_CAUSE);
    assert_eq!(version.summary.proposer.as_deref(), Some(PAGES_PROPOSER));
    assert_eq!(version.summary.confirmer.as_deref(), Some("person"));
    assert_eq!(version.summary.author, None);
    let shared = &version.rules[0];
    let shared_line = |text| {
        shared
            .text
            .lines()
            .zip(&shared.origins)
            .find(|(line, _)| *line == text)
            .unwrap()
            .1
            .as_ref()
            .unwrap()
    };
    let duplicate = shared_line("One rule.");
    assert!(duplicate.kept_shared);
    assert_eq!(
        duplicate.pages[0].location.page_id.as_deref(),
        Some("rule-page")
    );
    assert_eq!(duplicate.pages[0].location.line, 4);
    assert_eq!(duplicate.pages[0].author, None);
    assert!(shared
        .text
        .contains("## How I work\n\n- Check the original receipt."));
    assert!(!shared.text.contains(&generated[2]));
    let archive_path = fixture
        .setup
        .data
        .join(archive_path(&version.summary.id).unwrap());
    let archive = std::fs::read(&archive_path).unwrap();
    let pages: BTreeMap<String, Vec<String>> = serde_json::from_slice(&archive).unwrap();
    assert_eq!(pages["rule-page"], generated[..2]);

    // Passing the original generated data through the writer again models a
    // stale background read. Pullback must still see no second layer.
    assert!(fixture.sync_generated(&generated).blocked.is_empty());
    assert!(fixture.sync_generated(&generated).proposals.is_empty());
    for path in [CLAUDE, CODEX] {
        let delivered = std::fs::read_to_string(fixture.setup.home.join(path)).unwrap();
        for line in &generated {
            assert_eq!(delivered.matches(line).count(), 1);
        }
    }
    // After the move a new confirmed preference reaches every tool, and a
    // forgotten one leaves, while the moved page lines stay exactly once.
    let later = [generated[2].clone(), "Answer in short paragraphs.".into()];
    assert!(fixture.sync_generated(&later).blocked.is_empty());
    for path in [CLAUDE, CODEX] {
        let delivered = std::fs::read_to_string(fixture.setup.home.join(path)).unwrap();
        for line in generated.iter().chain(&later) {
            assert_eq!(
                delivered.matches(line.as_str()).count(),
                1,
                "{path}: {line}"
            );
        }
    }
    assert!(fixture.sync_generated(&later[1..]).blocked.is_empty());
    for path in [CLAUDE, CODEX] {
        let delivered = std::fs::read_to_string(fixture.setup.home.join(path)).unwrap();
        assert!(!delivered.contains(&generated[2]), "{path}");
        assert_eq!(
            delivered.matches(generated[1].as_str()).count(),
            1,
            "{path}"
        );
    }
    let manifest = fixture.setup.load().unwrap().unwrap();
    assert_eq!(manifest.page_rules, PageRulesState::Merged);
    assert!(manifest
        .master
        .overlays
        .values()
        .all(|text| !text.contains(&generated[1])));
    assert_eq!(
        read_version(&fixture.setup, &manifest, &version.summary.id)
            .unwrap()
            .rules[0]
            .origins,
        shared.origins
    );
    assert_eq!(std::fs::read(archive_path).unwrap(), archive);
}

#[test]
fn personal_rules_page_merge_stale_refusal_and_decline_keep_legacy_delivery() {
    use super::super::tests::CODEX;
    let fixture = adopted_fixture();
    let snapshot = PageRuleSnapshot {
        rules: vec![page_rule("Read the decision log.", "page", 2)],
        ..Default::default()
    };
    let generated = [snapshot.rules[0].text.clone()];
    fixture.sync_generated(&generated);
    let plan = preview_page_move(&fixture, snapshot.clone(), &[]);
    let mut changed = snapshot.clone();
    changed.revisions.insert("page".into(), 2);
    let result = fixture
        .setup
        .apply_plan(&plan.id, &[], Some(changed), &fixture_rule_renderer);
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("changed after the preview"));
    let plan = preview_page_move(&fixture, snapshot.clone(), &[]);
    fixture
        .setup
        .decline_page_plan(&plan.id, &snapshot)
        .unwrap();
    fixture.setup.mark_empty_page_rules().unwrap();
    let manifest = fixture.setup.load().unwrap().unwrap();
    assert_eq!(manifest.page_rules, PageRulesState::Declined);
    assert!(PagePlan::Merge(snapshot).prepare(&manifest).is_err());
    assert!(!manifest.master.shared.contains(&generated[0]));
    assert!(fixture.sync_generated(&generated).blocked.is_empty());
    assert!(std::fs::read_to_string(fixture.setup.home.join(CODEX))
        .unwrap()
        .contains(&generated[0]));
}

#[test]
fn personal_rules_page_merge_recovers_only_the_uncommitted_archive() {
    let fixture = Fixture::plain();
    let snapshot = PageRuleSnapshot {
        rules: vec![page_rule("Read the change log.", "page", 1)],
        ..Default::default()
    };
    let plan = preview_page_move(&fixture, snapshot.clone(), &[]);
    let version = fixture
        .setup
        .apply_plan(&plan.id, &[], Some(snapshot), &fixture_rule_renderer)
        .unwrap();
    let committed = fixture.setup.load().unwrap().unwrap();
    let files = committed
        .personal_rule_history
        .iter()
        .map(|id| {
            (
                id.clone(),
                std::fs::read(fixture.setup.data.join(history_path(id).unwrap())).unwrap(),
            )
        })
        .collect();
    let path = fixture
        .setup
        .data
        .join(archive_path(&version.summary.id).unwrap());
    let archive = std::fs::read(&path).unwrap();
    let mut interrupted = empty_manifest();
    interrupted.personal_rule_journal = Some(RuleHistoryJournal {
        files,
        archives: BTreeMap::from([(version.summary.id, archive.clone())]),
    });
    fixture.setup.save(&interrupted).unwrap();
    let root = fixture.setup.home_files().unwrap();
    std::fs::write(&path, "A newer archive copy.").unwrap();
    let error = fixture
        .setup
        .recover_pending(&root, &mut interrupted)
        .unwrap_err();
    assert!(error.to_string().contains("changed during recovery"));
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "A newer archive copy."
    );
    std::fs::write(&path, archive).unwrap();
    fixture
        .setup
        .recover_pending(&root, &mut interrupted)
        .unwrap();
    assert!(!path.exists());
    let restored = fixture.setup.load().unwrap().unwrap();
    assert!(restored.page_rules.is_unanswered());
    assert!(restored.personal_rule_journal.is_none());
    assert!(restored.master.shared.is_empty());
}

#[test]
fn personal_rules_empty_page_migration_works_with_one_or_no_tool() {
    use super::super::tests::CODEX;
    for one_tool in [false, true] {
        let fixture = Fixture::plain();
        if one_tool {
            std::fs::create_dir(fixture.setup.home.join(".codex")).unwrap();
            fixture.write(CODEX, "An existing local rule.\n");
        }
        fixture.setup.mark_empty_page_rules().unwrap();
        if one_tool {
            fixture.adopt_from_plan();
        }
        let manifest = fixture.setup.load().unwrap().unwrap();
        assert_eq!(manifest.page_rules, PageRulesState::Merged);
        assert!(manifest.personal_rule_history.is_empty());
        let snapshot = PageRuleSnapshot {
            rules: vec![page_rule("Use the locally recorded decision.", "page", 1)],
            ..Default::default()
        };
        fixture
            .setup
            .change_page_rules(&[], snapshot, BTreeSet::new(), true, &fixture_rule_renderer)
            .unwrap();
        let manifest = fixture.setup.load().unwrap().unwrap();
        assert!(manifest
            .master
            .shared
            .contains("Use the locally recorded decision."));
        if one_tool {
            let delivered = std::fs::read_to_string(fixture.setup.home.join(CODEX)).unwrap();
            assert!(delivered.contains("An existing local rule."));
            assert_eq!(
                delivered
                    .matches("Use the locally recorded decision.")
                    .count(),
                1
            );
        } else {
            assert_eq!(std::fs::read_dir(&fixture.setup.home).unwrap().count(), 0);
        }
    }
}

#[test]
fn rules_page_change_waits_while_a_moved_tool_file_has_an_unreviewed_edit() {
    use super::super::tests::CODEX;
    let fixture = Fixture::plain();
    std::fs::create_dir(fixture.setup.home.join(".codex")).unwrap();
    fixture.write(CODEX, "An existing local rule.\n");
    fixture.setup.mark_empty_page_rules().unwrap();
    fixture.adopt_from_plan();
    let mut manifest = fixture.setup.load().unwrap().unwrap();
    assert_eq!(manifest.page_rules, PageRulesState::Merged);
    pages::page_change_settled(&manifest).unwrap();
    let receipt = manifest
        .delivered
        .values_mut()
        .find(|receipt| matches!(receipt.kind, Kind::Rule { .. }))
        .unwrap();
    receipt.observed = Some(receipt.delivered.clone());
    let error = pages::page_change_settled(&manifest)
        .unwrap_err()
        .to_string();
    assert!(error.contains("needs review first"), "{error}");
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
fn personal_rule_plan_preserves_reviewed_tool_additions_on_setup_restore() {
    use super::super::tests::{accept_local_rule_additions, CLAUDE, CODEX};

    let fixture = Fixture::new();
    let original = "# Codex notes\r\n\r\nAn original rule.  \r\n";
    let additions = "\r\nSecond rule added.  \r\nFirst rule added.\nSecond rule added.\r\n";
    fixture.write(CODEX, original);
    let claude_original = fixture.text(CLAUDE);
    fixture.adopt_from_plan();
    accept_local_rule_additions(&fixture, additions);

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

    let fixture = adopted_fixture();
    let original = text(&fixture.setup.load().unwrap().unwrap().backup[Path::new(CODEX)])
        .unwrap()
        .to_owned();
    let addition = "A portable donor rule.\n";
    accept_local_rule_additions(&fixture, addition);
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
    assert!(restored.blocked.is_empty(), "{restored:?}");
    assert_eq!(fixture.text(CODEX), format!("{original}{addition}"));
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
    let before = fixture.setup.load().unwrap().unwrap();
    let plan = propose(
        &fixture,
        vec![PersonalRuleEdit {
            layer: PersonalRuleLayer::Shared,
            text: format!(
                "{}Another shared rule.\n",
                before.master.shared.replace(line, "")
            ),
        }],
    );
    apply(&fixture, &plan).unwrap();
    let mut stale = fixture.setup.load().unwrap().unwrap();
    stale.authored_rules = before.authored_rules;
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
    remember_plan(&fixture.setup, prepared, edits, restore, None)
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
fn person_confirmed_rule_share_records_one_change_with_person_confirmer() {
    let fixture = Fixture::new();
    let source = fixture.text(SHARED_RULES);
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

fn check_personal_rule_batch_rollback() {
    let fixture = adopted_fixture();
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
    check_personal_rule_batch_rollback();
}

fn check_personal_rule_interrupted_recovery() {
    let fixture = adopted_fixture();
    super::super::tests::accept_local_rule_additions(&fixture, "An approved donor rule.\n");
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
    check_personal_rule_interrupted_recovery();
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
fn confirmed_personal_tool_rule_survives_setup_restore() {
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
    assert_eq!(confirmed.personal_rule_history.len(), 2);

    let report = fixture.setup.restore().unwrap();
    assert!(report.blocked.is_empty(), "{report:?}");
    let restored = fixture.setup.load().unwrap().unwrap();
    assert_eq!(restored.master.overlays[CLAUDE_RULES], next_overlay);
    assert_eq!(
        restored.personal_rule_history,
        confirmed.personal_rule_history
    );
    assert!(restored.master.unenrolled_overlays.contains(CLAUDE_RULES));
    assert!(restored.personal_rule_journal.is_none());
}
