// SPDX-License-Identifier: Apache-2.0
use crate::{
    lint::{
        canonical_check_ids, canonical_gate_effect, LintAgentWork, LintApplicability,
        LintCapabilityContext, LintCheckResult, LintCheckResultInput, LintConfigFingerprint,
        LintCoverage, LintDbSnapshotMode, LintDbSnapshotReceipt, LintDigest, LintEvidenceRef,
        LintGateEffect, LintOpaqueId, LintOutcome, LintPageSnapshotMode, LintPageSnapshotReceipt,
        LintPrecondition, LintProducerReceipt, LintProfile, LintScope, LintSemanticAction,
        LintSemanticCheckId, LintSemanticFinding, LintSemanticPopulation,
        LintSemanticProviderRoute, LintSemanticReasonCode, LintSeverity, LintSnapshotReceipts,
        LintSummaryCode, LintValidationMethod,
    },
    repair::{
        ApplyRepairRequest, PrepareRepairRequest, RepairAllowedEffects, RepairApplyReceipt,
        RepairApplyReceiptDraft, RepairCheckBaseline, RepairChoice, RepairDigest,
        RepairEnrichmentStep, RepairExpectedState, RepairLintScope, RepairManifest,
        RepairManifestDraft, RepairMutation, RepairPostAssertions, RepairReviewBinding,
        RepairRollbackArtifact, RepairRollbackFileEntry, RepairRollbackPayloadV2, RepairRollbackV2,
        RepairScope, RepairSource, RepairTarget, RepairVerificationReceipt,
        RepairVerificationReceiptDraft, RepairWriter, StoredRepairApplyReceipt,
        StoredRepairManifest, StoredRepairRollbackArtifact, StoredRepairVerificationReceipt,
        VerifyRepairRequest,
    },
    LintReport, MemoryType,
};

const SHA256_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const SHA256_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const GOLDEN_ROLLBACK_DIGEST: &str =
    "13f263e12b211c197519cea152705c573fc21c2352cbca6f757043a04feae623";
const GOLDEN_PRE_BASELINE_MANIFEST_DIGEST: &str =
    "f1e21fb246b6d1bda32b9620798241d53e590adce683e231179f01e9f00599c1";
const GOLDEN_MANIFEST_DIGEST: &str =
    "6d79617ffac084a9668025d2a870aa569b5381ea62513c4fa57d9f1a1620bf34";
const GOLDEN_APPLY_RECEIPT_DIGEST: &str =
    "2c0107e820d5b6425effe0177d4a676835fe2b2bd11f7f69ee437d04ea085169";
const GOLDEN_VERIFICATION_RECEIPT_DIGEST: &str =
    "3377e9eb4aa9d1a1e1441e3df0aa6fed6446e64a1423f8525e6653d6acb05f2d";

fn repair_test_sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};

    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn strip_current_baseline_counts(value: &mut serde_json::Value) {
    for baseline_key in ["general_baseline", "deep_baseline"] {
        if let Some(baselines) = value["post_assertions"][baseline_key].as_array_mut() {
            for baseline in baselines {
                baseline
                    .as_object_mut()
                    .expect("repair baseline is an object")
                    .remove("affected_records");
            }
        }
    }
}

fn frozen_v1_architecture_violations(source: &str) -> Vec<String> {
    use std::collections::BTreeSet;
    use syn::{visit::Visit, Fields, GenericArgument, Item, PathArguments, Type, UseTree};

    struct FrozenPathVisitor<'a> {
        violations: &'a mut Vec<String>,
    }

    impl<'ast> Visit<'ast> for FrozenPathVisitor<'_> {
        fn visit_path(&mut self, path: &'ast syn::Path) {
            if path.segments.first().is_some_and(|segment| {
                segment.ident == "crate"
                    || (path.segments.len() > 1
                        && matches!(segment.ident.to_string().as_str(), "super" | "self"))
            }) {
                self.violations.push(format!(
                    "live path anywhere in frozen module: {}",
                    path.segments[0].ident
                ));
            }
            syn::visit::visit_path(self, path);
        }

        fn visit_item_mod(&mut self, item: &'ast syn::ItemMod) {
            self.violations
                .push(format!("submodule in frozen graph: {}", item.ident));
            syn::visit::visit_item_mod(self, item);
        }
    }

    fn declared_frozen_types(file: &syn::File) -> BTreeSet<String> {
        file.items
            .iter()
            .filter_map(|item| match item {
                Item::Struct(item) => Some(item.ident.to_string()),
                Item::Enum(item) => Some(item.ident.to_string()),
                _ => None,
            })
            .filter(|name| name.starts_with("Frozen") && name.ends_with("V1"))
            .collect()
    }

    fn inspect_use(tree: &UseTree, root: bool, violations: &mut Vec<String>) {
        match tree {
            UseTree::Path(path) => {
                if root && matches!(path.ident.to_string().as_str(), "crate" | "super" | "self") {
                    violations.push(format!("live path import: {}", path.ident));
                }
                inspect_use(&path.tree, false, violations);
            }
            UseTree::Group(group) => {
                for item in &group.items {
                    inspect_use(item, root, violations);
                }
            }
            UseTree::Glob(_) => violations.push("glob import".to_string()),
            UseTree::Name(_) | UseTree::Rename(_) => {}
        }
    }

    fn inspect_type(ty: &Type, declared: &BTreeSet<String>, violations: &mut Vec<String>) {
        match ty {
            Type::Path(path) => {
                if path.qself.is_some() {
                    violations.push("qualified live path in frozen field".to_string());
                }
                let Some(first) = path.path.segments.first() else {
                    violations.push("empty type path in frozen field".to_string());
                    return;
                };
                if matches!(first.ident.to_string().as_str(), "crate" | "super" | "self") {
                    violations.push(format!("live path in frozen field: {}", first.ident));
                }
                for segment in &path.path.segments {
                    if let PathArguments::AngleBracketed(arguments) = &segment.arguments {
                        for argument in &arguments.args {
                            if let GenericArgument::Type(inner) = argument {
                                inspect_type(inner, declared, violations);
                            }
                        }
                    }
                }
                let leaf = path.path.segments.last().unwrap().ident.to_string();
                let allowed_leaf = matches!(
                    leaf.as_str(),
                    "String"
                        | "Vec"
                        | "Option"
                        | "bool"
                        | "i8"
                        | "i16"
                        | "i32"
                        | "i64"
                        | "i128"
                        | "isize"
                        | "u8"
                        | "u16"
                        | "u32"
                        | "u64"
                        | "u128"
                        | "usize"
                ) || declared.contains(&leaf);
                if !allowed_leaf {
                    violations.push(format!("non-frozen field leaf: {leaf}"));
                }
            }
            Type::Array(array) => inspect_type(&array.elem, declared, violations),
            Type::Group(group) => inspect_type(&group.elem, declared, violations),
            Type::Paren(paren) => inspect_type(&paren.elem, declared, violations),
            Type::Ptr(pointer) => inspect_type(&pointer.elem, declared, violations),
            Type::Reference(reference) => inspect_type(&reference.elem, declared, violations),
            Type::Slice(slice) => inspect_type(&slice.elem, declared, violations),
            Type::Tuple(tuple) => {
                for element in &tuple.elems {
                    inspect_type(element, declared, violations);
                }
            }
            unsupported => violations.push(format!(
                "unsupported frozen field type: {:?}",
                std::mem::discriminant(unsupported)
            )),
        }
    }

    fn inspect_fields(fields: &Fields, declared: &BTreeSet<String>, violations: &mut Vec<String>) {
        for field in fields {
            inspect_type(&field.ty, declared, violations);
        }
    }

    let file = match syn::parse_file(source) {
        Ok(file) => file,
        Err(error) => return vec![format!("frozen_v1 parse failed: {error}")],
    };
    let declared = declared_frozen_types(&file);
    let mut violations = Vec::new();
    FrozenPathVisitor {
        violations: &mut violations,
    }
    .visit_file(&file);
    for item in &file.items {
        match item {
            Item::Use(item) => inspect_use(&item.tree, true, &mut violations),
            Item::Type(item) => violations.push(format!("type alias: {}", item.ident)),
            Item::Struct(item)
                if item.ident.to_string().starts_with("Frozen")
                    && item.ident.to_string().ends_with("V1") =>
            {
                inspect_fields(&item.fields, &declared, &mut violations);
            }
            Item::Enum(item)
                if item.ident.to_string().starts_with("Frozen")
                    && item.ident.to_string().ends_with("V1") =>
            {
                for variant in &item.variants {
                    inspect_fields(&variant.fields, &declared, &mut violations);
                }
            }
            Item::Struct(item) => {
                violations.push(format!("non-frozen struct in frozen graph: {}", item.ident))
            }
            Item::Enum(item) => {
                violations.push(format!("non-frozen enum in frozen graph: {}", item.ident))
            }
            Item::Fn(item) => violations.push(format!(
                "top-level function in frozen graph: {}",
                item.sig.ident
            )),
            _ => {}
        }
    }
    violations.sort();
    violations.dedup();
    violations
}

#[derive(Debug, PartialEq, Eq)]
struct SerdeEnumContract {
    serde_attributes: Vec<String>,
    variants: Vec<(String, Vec<String>)>,
}

fn serde_enum_contract(source: &str, enum_name: &str) -> SerdeEnumContract {
    fn serde_attributes(attributes: &[syn::Attribute]) -> Vec<String> {
        attributes
            .iter()
            .filter(|attribute| attribute.path().is_ident("serde"))
            .map(|attribute| match &attribute.meta {
                syn::Meta::List(list) => list.tokens.to_string(),
                _ => panic!("unexpected serde attribute shape"),
            })
            .collect()
    }

    let file = syn::parse_file(source).expect("contract source parses");
    let item = file
        .items
        .iter()
        .find_map(|item| match item {
            syn::Item::Enum(item) if item.ident == enum_name => Some(item),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing enum {enum_name}"));
    SerdeEnumContract {
        serde_attributes: serde_attributes(&item.attrs),
        variants: item
            .variants
            .iter()
            .map(|variant| (variant.ident.to_string(), serde_attributes(&variant.attrs)))
            .collect(),
    }
}

fn snapshots(seed: u64) -> LintSnapshotReceipts {
    LintSnapshotReceipts::new(
        LintDbSnapshotReceipt::new(
            LintDbSnapshotMode::TransactionalReadOnly,
            LintDigest::from_u64(seed),
            Some(LintDigest::from_u64(seed)),
        ),
        LintPageSnapshotReceipt::new(
            LintPageSnapshotMode::BestEffort,
            LintDigest::from_u64(seed + 1),
            Some(LintDigest::from_u64(seed + 1)),
        ),
    )
}

fn complete_report(
    profile: LintProfile,
    scope: LintScope,
    producer_receipt: LintProducerReceipt,
    seed: u64,
) -> LintReport {
    let checks = canonical_check_ids(profile)
        .map(|check_id| {
            LintCheckResult::try_new_with_gate_effect(
                LintCheckResultInput {
                    check_id: check_id.to_string(),
                    outcome: LintOutcome::Pass,
                    severity: LintSeverity::Info,
                    applicability: LintApplicability::Applicable,
                    precondition: LintPrecondition::Ready,
                    coverage: LintCoverage::new(
                        LintValidationMethod::FullEnumeration,
                        0,
                        0,
                        100,
                        false,
                        0,
                    )
                    .unwrap(),
                    metrics: vec![],
                    summary_code: LintSummaryCode::CheckPassed,
                    recommendation_code: None,
                    evidence: vec![],
                    duration_ms: 0,
                },
                canonical_gate_effect(profile, check_id).unwrap(),
            )
            .unwrap()
        })
        .collect();
    LintReport::try_new_for_profile(
        profile,
        scope,
        LintCapabilityContext::daemon_operator_endpoint(),
        snapshots(seed),
        LintConfigFingerprint::from_effective_config(&[]),
        producer_receipt,
        checks,
    )
    .unwrap()
}

fn baselines(
    semantic_finding: Option<LintSemanticFinding>,
) -> (Vec<RepairCheckBaseline>, Vec<RepairCheckBaseline>) {
    let mut deep_evidence = vec![LintEvidenceRef::ReasonCode {
        reason_code: crate::lint::LintReasonCode::SemanticAgentAdjudicationRequired,
    }];
    if let Some(finding) = semantic_finding {
        deep_evidence.push(LintEvidenceRef::SemanticFinding { finding });
    }
    (
        vec![RepairCheckBaseline::try_new_current(
            "memories.structural.integrity".into(),
            LintOutcome::Pass,
            LintGateEffect::Actionable,
            0,
            vec![],
        )
        .unwrap()],
        vec![RepairCheckBaseline::try_new_current(
            "memories.semantic.classification".into(),
            LintOutcome::Finding,
            LintGateEffect::Actionable,
            1,
            deep_evidence,
        )
        .unwrap()],
    )
}

fn fixture_manifest() -> RepairManifest {
    let evidence_id = LintDigest::from_u64(42);
    let finding = LintSemanticFinding::try_new(
        LintOpaqueId::from_sorted_position(0).unwrap(),
        LintSemanticAction::ReclassifyMemory,
        LintSemanticReasonCode::ClassificationMismatch,
        9_000,
        LintSemanticProviderRoute::CallingAgent,
        vec![evidence_id.clone()],
        vec![],
    )
    .unwrap();
    let source = RepairSource::try_new(
        RepairLintScope::global(),
        LintScope::global(),
        finding.clone(),
        snapshots(1),
        snapshots(3),
        LintProducerReceipt::new(None),
        LintProducerReceipt::new(None),
        LintDigest::from_u64(5),
    )
    .unwrap();
    let target = RepairTarget::memory(
        "mem_target".into(),
        RepairScope::registered("work".into()).unwrap(),
    )
    .unwrap();
    let (general_baseline, deep_baseline) = baselines(Some(finding));
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        1_721_000_000,
        source,
        target.clone(),
        RepairExpectedState::try_new(None, RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::ReclassifyMemory,
        RepairMutation::try_reclassify(Some("fact"), "decision").unwrap(),
        RepairAllowedEffects::memory_type(target),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(GOLDEN_ROLLBACK_DIGEST).unwrap(),
        )
        .unwrap(),
        RepairPostAssertions::try_new(evidence_id, general_baseline, deep_baseline, vec![])
            .unwrap(),
    )
    .unwrap();
    let digest =
        RepairDigest::parse(&repair_test_sha256(&draft.canonical_bytes().unwrap())).unwrap();
    RepairManifest::try_new(draft, digest).unwrap()
}

#[test]
fn new_reclassification_manifest_binds_the_durable_review_owner() {
    let evidence_id = LintDigest::from_u64(42);
    let finding = LintSemanticFinding::try_new(
        LintOpaqueId::from_sorted_position(0).unwrap(),
        LintSemanticAction::ReclassifyMemory,
        LintSemanticReasonCode::ClassificationMismatch,
        9_000,
        LintSemanticProviderRoute::CallingAgent,
        vec![evidence_id.clone()],
        vec![],
    )
    .unwrap();
    let review_binding = RepairReviewBinding::try_new(
        format!("lint_review_{SHA256_A}"),
        RepairDigest::parse(SHA256_A).unwrap(),
        vec!["mem_target".into()],
    )
    .unwrap();
    let source = RepairSource::try_new(
        RepairLintScope::global(),
        LintScope::global(),
        finding.clone(),
        snapshots(1),
        snapshots(3),
        LintProducerReceipt::new(None),
        LintProducerReceipt::new(None),
        LintDigest::from_u64(5),
    )
    .unwrap()
    .try_with_review_binding(review_binding)
    .unwrap();
    let target = RepairTarget::memory(
        "mem_target".into(),
        RepairScope::registered("work".into()).unwrap(),
    )
    .unwrap();
    let (general_baseline, deep_baseline) = baselines(Some(finding));
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440099".into(),
        1_721_000_000,
        source,
        target.clone(),
        RepairExpectedState::try_new(None, RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::ReclassifyMemory,
        RepairMutation::try_reclassify(Some("fact"), "decision").unwrap(),
        RepairAllowedEffects::memory_type(target),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(GOLDEN_ROLLBACK_DIGEST).unwrap(),
        )
        .unwrap(),
        RepairPostAssertions::try_new(evidence_id, general_baseline, deep_baseline, vec![])
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(draft).unwrap()["manifest_schema_version"],
        serde_json::json!(6)
    );
}

fn persisted_fixture(contents: &'static str) -> &'static [u8] {
    contents.strip_suffix('\n').unwrap_or(contents).as_bytes()
}

#[test]
fn repair_v1_manifest_decodes_through_frozen_envelope_without_byte_drift() {
    let fixture = persisted_fixture(include_str!("../testdata/repair/v1/manifest.json"));
    let stored = StoredRepairManifest::from_slice(fixture).expect("v1 manifest remains readable");

    assert!(matches!(stored, StoredRepairManifest::V1(_)));
    assert_eq!(
        stored.manifest_id(),
        "repair_550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(stored.manifest_digest().as_str(), GOLDEN_MANIFEST_DIGEST);
    assert_eq!(stored.persisted_bytes().unwrap(), fixture);
    assert!(stored
        .clone()
        .verify_and_try_into_current(|_, _| false)
        .is_err());
    let current = stored
        .clone()
        .verify_and_try_into_current(|canonical, expected| {
            repair_test_sha256(canonical) == expected.as_str()
        })
        .expect("verified v1 manifest maps explicitly to current operation values");
    assert_eq!(current.target().memory_source_id(), "mem_target");
    assert!(current
        .post_assertions()
        .verification_policy()
        .requires_whole_reports());

    let canonical_bytes = stored.canonical_unsigned_bytes().unwrap();
    assert_eq!(repair_test_sha256(&canonical_bytes), GOLDEN_MANIFEST_DIGEST);
    let compact_fixture =
        serde_json::to_vec(&serde_json::from_slice::<serde_json::Value>(fixture).unwrap()).unwrap();
    let compact_stored = StoredRepairManifest::from_slice(&compact_fixture).unwrap();
    assert_eq!(
        compact_stored.canonical_unsigned_bytes().unwrap(),
        canonical_bytes,
        "digest canonicalization is independent of persisted JSON whitespace"
    );
    let rollback_fixture = persisted_fixture(include_str!("../testdata/repair/v1/rollback.json"));
    assert_eq!(
        repair_test_sha256(rollback_fixture),
        stored.rollback_digest().as_str()
    );
    let canonical: serde_json::Value = serde_json::from_slice(&canonical_bytes).unwrap();
    assert!(canonical.get("manifest_digest").is_none());
    assert_eq!(
        canonical["post_assertions"]["deep_baseline"][0]["evidence"][1]["kind"],
        "semantic_finding"
    );
}

#[test]
fn repair_v2_manifest_roundtrips_through_current_envelope() {
    let manifest = fixture_manifest();
    let mut value = serde_json::to_value(manifest).unwrap();
    value["manifest_schema_version"] = serde_json::json!(2);
    value["source"]["report_schema_version"] = serde_json::json!(4);
    value["source"]["check_catalog_version"] = serde_json::json!(2);
    strip_current_baseline_counts(&mut value);
    value["manifest_digest"] = serde_json::json!(SHA256_A);
    let provisional =
        StoredRepairManifest::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
    let v2_canonical = provisional.canonical_unsigned_bytes().unwrap();
    value["manifest_digest"] = serde_json::json!(repair_test_sha256(&v2_canonical));
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    let stored = StoredRepairManifest::from_slice(&bytes).unwrap();

    assert!(matches!(stored, StoredRepairManifest::V2(_)));
    let canonical = stored.canonical_unsigned_bytes().unwrap();
    assert_eq!(
        repair_test_sha256(&canonical),
        stored.manifest_digest().as_str()
    );
    let current = stored
        .verify_and_try_into_current(|canonical, expected| {
            repair_test_sha256(canonical) == expected.as_str()
        })
        .unwrap();
    assert_eq!(serde_json::to_value(&current).unwrap(), value);
    assert!(current.source().deep_snapshots().is_some());
    assert_eq!(
        current
            .post_assertions()
            .verification_policy()
            .required_deep_check_ids()
            .unwrap(),
        ["memories.semantic.classification"]
    );
}

#[test]
fn repair_v3_manifest_roundtrips_without_reinterpreting_v4_fields() {
    let manifest = fixture_manifest();
    let mut value = serde_json::to_value(manifest).unwrap();
    value["manifest_schema_version"] = serde_json::json!(3);
    strip_current_baseline_counts(&mut value);
    value["manifest_digest"] = serde_json::json!(SHA256_A);
    let provisional =
        StoredRepairManifest::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap();
    let v3_canonical = provisional.canonical_unsigned_bytes().unwrap();
    value["manifest_digest"] = serde_json::json!(repair_test_sha256(&v3_canonical));
    let bytes = serde_json::to_vec_pretty(&value).unwrap();
    let stored = StoredRepairManifest::from_slice(&bytes).unwrap();

    assert!(matches!(stored, StoredRepairManifest::V3(_)));
    assert_eq!(stored.canonical_unsigned_bytes().unwrap(), v3_canonical);
    assert_eq!(
        repair_test_sha256(&stored.canonical_unsigned_bytes().unwrap()),
        stored.manifest_digest().as_str()
    );
    let current = stored
        .verify_and_try_into_current(|canonical, expected| {
            repair_test_sha256(canonical) == expected.as_str()
        })
        .unwrap();
    assert_eq!(serde_json::to_value(current).unwrap(), value);
}

#[test]
fn repair_v3_manifest_rejects_v4_record_set_contract() {
    let manifest = fixture_manifest();
    let mut value = serde_json::to_value(manifest).unwrap();
    value["manifest_schema_version"] = serde_json::json!(3);
    value["post_assertions"]["target_record_set"] = serde_json::json!({
        "record_count": 1,
        "digest": SHA256_A,
    });

    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&value).unwrap()).is_err(),
        "v3 must not silently reinterpret the exact-set contract introduced in v4"
    );
}

#[test]
fn repair_manifest_schema_rejects_crossed_lint_report_versions() {
    let manifest = fixture_manifest();

    let mut v3_with_v4_lint = serde_json::to_value(&manifest).unwrap();
    v3_with_v4_lint["source"]["report_schema_version"] = serde_json::json!(4);
    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&v3_with_v4_lint).unwrap()).is_err()
    );

    let mut v2_with_v5_lint = serde_json::to_value(manifest).unwrap();
    v2_with_v5_lint["manifest_schema_version"] = serde_json::json!(2);
    v2_with_v5_lint["source"]["report_schema_version"] = serde_json::json!(5);
    strip_current_baseline_counts(&mut v2_with_v5_lint);
    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&v2_with_v5_lint).unwrap()).is_err()
    );
}

#[test]
fn repair_manifest_v2_rejects_v3_affected_counts() {
    let manifest = fixture_manifest();
    let mut v2_with_counts = serde_json::to_value(manifest).unwrap();
    v2_with_counts["manifest_schema_version"] = serde_json::json!(2);
    v2_with_counts["source"]["report_schema_version"] = serde_json::json!(4);
    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&v2_with_counts).unwrap()).is_err()
    );
}

#[test]
fn repair_v2_manifest_rejects_v5_only_opaque_digest_evidence() {
    let manifest = fixture_manifest();
    let mut baseline_value = serde_json::to_value(&manifest).unwrap();
    baseline_value["manifest_schema_version"] = serde_json::json!(2);
    baseline_value["source"]["report_schema_version"] = serde_json::json!(4);
    strip_current_baseline_counts(&mut baseline_value);
    baseline_value["post_assertions"]["general_baseline"][0]["evidence"] = serde_json::json!([{
        "kind": "opaque_digest",
        "opaque_digest": SHA256_A,
    }]);
    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&baseline_value).unwrap()).is_err(),
        "v2 baseline must not accept an evidence variant introduced in report schema v5"
    );

    let source = RepairSource::try_new_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        "identity.memory_state_integrity".into(),
        vec![serde_json::from_value(serde_json::json!({
            "kind": "opaque_digest",
            "opaque_digest": SHA256_A,
        }))
        .unwrap()],
        snapshots(1),
        snapshots(3),
        LintProducerReceipt::new(None),
        LintProducerReceipt::new(None),
    )
    .unwrap();
    let target = RepairTarget::memory("mem_target".into(), RepairScope::uncategorized()).unwrap();
    let baseline = vec![RepairCheckBaseline::try_new_current(
        "identity.memory_state_integrity".into(),
        LintOutcome::Finding,
        LintGateEffect::Actionable,
        1,
        vec![],
    )
    .unwrap()];
    let assertions = RepairPostAssertions::try_new_for_check(
        "identity.memory_state_integrity".into(),
        LintDigest::from_u64(8),
        baseline.clone(),
        baseline,
        vec![],
        vec![],
    )
    .unwrap();
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440099".into(),
        1_721_000_000,
        source,
        target.clone(),
        RepairExpectedState::try_new(None, RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::NormalizeMemorySourceAgent,
        RepairMutation::normalize_memory_source_agent(" ".into()).unwrap(),
        RepairAllowedEffects::memory_source_agent(target),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(SHA256_B).unwrap(),
        )
        .unwrap(),
        assertions,
    )
    .unwrap();
    let manifest = RepairManifest::try_new(draft, RepairDigest::parse(SHA256_B).unwrap()).unwrap();
    let mut source_value = serde_json::to_value(manifest).unwrap();
    source_value["manifest_schema_version"] = serde_json::json!(2);
    source_value["source"]["report_schema_version"] = serde_json::json!(4);
    strip_current_baseline_counts(&mut source_value);
    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&source_value).unwrap()).is_err(),
        "v2 source must not accept an evidence variant introduced in report schema v5"
    );
}

#[test]
fn repair_v1_pre_baseline_manifest_remains_loadable_with_original_digest() {
    let fixture = persisted_fixture(include_str!(
        "../testdata/repair/v1/manifest-pre-baseline.json"
    ));
    let stored =
        StoredRepairManifest::from_slice(fixture).expect("first durable v1 shape remains readable");

    assert!(matches!(stored, StoredRepairManifest::V1PreBaseline(_)));
    assert_eq!(
        stored.manifest_digest().as_str(),
        GOLDEN_PRE_BASELINE_MANIFEST_DIGEST
    );
    assert_eq!(stored.persisted_bytes().unwrap(), fixture);
    let canonical = stored.canonical_unsigned_bytes().unwrap();
    assert_eq!(
        repair_test_sha256(&canonical),
        GOLDEN_PRE_BASELINE_MANIFEST_DIGEST
    );
    let current = stored
        .verify_and_try_into_current(|bytes, expected| {
            repair_test_sha256(bytes) == expected.as_str()
        })
        .expect("verified historical v1 maps to conservative live operation values");
    assert!(current.post_assertions().general_baseline().is_empty());
    assert!(current.post_assertions().deep_baseline().is_empty());
}

#[test]
fn repair_v1_rollback_and_receipts_decode_without_byte_drift() {
    let rollback_fixture = persisted_fixture(include_str!("../testdata/repair/v1/rollback.json"));
    let rollback = StoredRepairRollbackArtifact::from_slice(rollback_fixture)
        .expect("v1 rollback remains readable");
    assert!(matches!(rollback, StoredRepairRollbackArtifact::V1(_)));
    assert_eq!(rollback.persisted_bytes().unwrap(), rollback_fixture);

    let apply_fixture = persisted_fixture(include_str!("../testdata/repair/v1/apply-receipt.json"));
    let apply = StoredRepairApplyReceipt::from_slice(apply_fixture)
        .expect("v1 apply receipt remains readable");
    assert!(matches!(apply, StoredRepairApplyReceipt::V1(_)));
    assert_eq!(apply.receipt_digest().as_str(), GOLDEN_APPLY_RECEIPT_DIGEST);
    assert_eq!(apply.persisted_bytes().unwrap(), apply_fixture);
    let current_apply = apply
        .clone()
        .verify_and_try_into_current(|canonical, expected| {
            repair_test_sha256(canonical) == expected.as_str()
        })
        .expect("verified v1 apply receipt maps explicitly");
    assert_eq!(
        current_apply.manifest_id(),
        "repair_550e8400-e29b-41d4-a716-446655440000"
    );
    let public_apply_bytes = serde_json::to_vec(&current_apply).unwrap();
    let public_apply: RepairApplyReceipt = serde_json::from_slice(&public_apply_bytes)
        .expect("HTTP/MCP public apply type roundtrips v1");
    assert_eq!(public_apply, current_apply);
    assert_eq!(
        repair_test_sha256(&public_apply.canonical_unsigned_bytes().unwrap()),
        public_apply.receipt_digest().as_str()
    );
    let apply_canonical_bytes = apply.canonical_unsigned_bytes().unwrap();
    assert_eq!(
        repair_test_sha256(&apply_canonical_bytes),
        GOLDEN_APPLY_RECEIPT_DIGEST
    );
    let apply_canonical: serde_json::Value =
        serde_json::from_slice(&apply_canonical_bytes).unwrap();
    assert!(apply_canonical.get("receipt_digest").is_none());

    let verification_fixture = persisted_fixture(include_str!(
        "../testdata/repair/v1/verification-receipt.json"
    ));
    let verification = StoredRepairVerificationReceipt::from_slice(verification_fixture)
        .expect("v1 verification receipt remains readable");
    assert!(matches!(
        verification,
        StoredRepairVerificationReceipt::V1(_)
    ));
    assert_eq!(
        verification.receipt_digest().as_str(),
        GOLDEN_VERIFICATION_RECEIPT_DIGEST
    );
    assert_eq!(
        verification.persisted_bytes().unwrap(),
        verification_fixture
    );
    let current_verification = verification
        .clone()
        .verify_and_try_into_current(|canonical, expected| {
            repair_test_sha256(canonical) == expected.as_str()
        })
        .expect("verified v1 verification receipt maps explicitly");
    assert_eq!(
        current_verification.manifest_id(),
        "repair_550e8400-e29b-41d4-a716-446655440000"
    );
    let public_verification_bytes = serde_json::to_vec(&current_verification).unwrap();
    let public_verification: RepairVerificationReceipt =
        serde_json::from_slice(&public_verification_bytes)
            .expect("HTTP/MCP public verification type roundtrips v1");
    assert_eq!(public_verification, current_verification);
    assert_eq!(
        repair_test_sha256(&public_verification.canonical_unsigned_bytes().unwrap()),
        public_verification.receipt_digest().as_str()
    );
    let verification_canonical_bytes = verification.canonical_unsigned_bytes().unwrap();
    assert_eq!(
        repair_test_sha256(&verification_canonical_bytes),
        GOLDEN_VERIFICATION_RECEIPT_DIGEST
    );
    let verification_canonical: serde_json::Value =
        serde_json::from_slice(&verification_canonical_bytes).unwrap();
    assert!(verification_canonical.get("receipt_digest").is_none());
}

#[test]
fn repair_v1_tagged_contracts_reject_unknown_fields_before_digest_verification() {
    let fixture = include_str!("../testdata/repair/v1/manifest.json");
    let mut lint_scope = serde_json::from_str::<serde_json::Value>(fixture).unwrap();
    lint_scope["source"]["lint_scope"]["unexpected"] = serde_json::json!(true);
    let mut target = serde_json::from_str::<serde_json::Value>(fixture).unwrap();
    target["target"]["unexpected"] = serde_json::json!(true);
    let mut target_scope = serde_json::from_str::<serde_json::Value>(fixture).unwrap();
    target_scope["target"]["scope"]["unexpected"] = serde_json::json!(true);
    let mut mutation = serde_json::from_str::<serde_json::Value>(fixture).unwrap();
    mutation["mutation"]["unexpected"] = serde_json::json!(true);

    for (label, invalid) in [
        ("lint_scope", lint_scope),
        ("target", target),
        ("target_scope", target_scope),
        ("mutation", mutation),
    ] {
        assert!(
            StoredRepairManifest::from_slice(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "{label} accepted an unknown field"
        );
    }
}

#[test]
fn repair_v1_outer_contracts_reject_unknown_fields_before_digest_verification() {
    for fixture in [
        include_str!("../testdata/repair/v1/manifest.json"),
        include_str!("../testdata/repair/v1/manifest-pre-baseline.json"),
    ] {
        let mut invalid = serde_json::from_str::<serde_json::Value>(fixture).unwrap();
        invalid["unexpected"] = serde_json::json!(true);
        assert!(
            StoredRepairManifest::from_slice(&serde_json::to_vec(&invalid).unwrap()).is_err(),
            "manifest wrapper accepted an unknown top-level field"
        );
    }

    let mut apply = serde_json::from_str::<serde_json::Value>(include_str!(
        "../testdata/repair/v1/apply-receipt.json"
    ))
    .unwrap();
    apply["unexpected"] = serde_json::json!(true);
    assert!(
        StoredRepairApplyReceipt::from_slice(&serde_json::to_vec(&apply).unwrap()).is_err(),
        "apply receipt wrapper accepted an unknown top-level field"
    );

    let mut verification = serde_json::from_str::<serde_json::Value>(include_str!(
        "../testdata/repair/v1/verification-receipt.json"
    ))
    .unwrap();
    verification["unexpected"] = serde_json::json!(true);
    assert!(
        StoredRepairVerificationReceipt::from_slice(&serde_json::to_vec(&verification).unwrap())
            .is_err(),
        "verification receipt wrapper accepted an unknown top-level field"
    );
}

#[test]
fn repair_frozen_v1_mirrored_enum_contracts_match_live_v1() {
    let frozen = include_str!("repair/frozen_v1.rs");
    let lint_agent = include_str!("lint_agent.rs");
    let lint_catalog = include_str!("lint_catalog.rs");
    for (live_source, live_name, frozen_name) in [
        (
            lint_agent,
            "LintSemanticAction",
            "FrozenLintSemanticActionV1",
        ),
        (
            lint_agent,
            "LintSemanticReasonCode",
            "FrozenLintSemanticReasonCodeV1",
        ),
        (lint_catalog, "LintReasonCode", "FrozenLintReasonCodeV1"),
        (
            lint_catalog,
            "LintSafeRootRelativePath",
            "FrozenLintSafeRootRelativePathV1",
        ),
    ] {
        assert_eq!(
            serde_enum_contract(frozen, frozen_name),
            serde_enum_contract(live_source, live_name),
            "frozen v1 enum drifted from live v1 contract: {live_name}"
        );
    }
}

#[test]
fn repair_frozen_v1_architecture_rejects_live_type_reachability() {
    let invalid = r#"
        use crate::lint::LintDigest;
        type FrozenAliasV1 = LintDigest;
        struct FrozenBadV1 { digest: crate::lint::LintDigest }
        impl FrozenBadV1 {
            fn into_live(&self) -> crate::repair::RepairManifest { panic!() }
        }
        mod hidden {}
    "#;
    let violations = frozen_v1_architecture_violations(invalid);
    assert!(violations.iter().any(|item| item.contains("live path")));
    assert!(violations.iter().any(|item| item.contains("type alias")));
    assert!(violations.iter().any(|item| item.contains("submodule")));

    let actual = include_str!("repair/frozen_v1.rs");
    assert_eq!(
        frozen_v1_architecture_violations(actual),
        Vec::<String>::new()
    );
}

#[test]
fn post_assertion_baselines_are_sorted_unique_and_strict() {
    let (general, deep) = baselines(None);
    let assertions =
        RepairPostAssertions::try_new(LintDigest::from_u64(42), general.clone(), deep, vec![])
            .unwrap();
    assert_eq!(assertions.general_baseline(), general);

    let duplicate = vec![general[0].clone(), general[0].clone()];
    assert!(
        RepairPostAssertions::try_new(LintDigest::from_u64(42), duplicate, vec![], vec![],)
            .is_err()
    );

    let mut value = serde_json::to_value(assertions).unwrap();
    value["general_baseline"][0]["hidden_content"] = serde_json::json!("secret");
    assert!(serde_json::from_value::<RepairPostAssertions>(value).is_err());
}

#[test]
fn repair_digest_requires_exact_lowercase_sha256() {
    let digest = RepairDigest::parse(SHA256_A).expect("valid digest");
    assert_eq!(digest.as_str(), SHA256_A);

    for invalid in [
        "aaaaaaaaaaaaaaaa",
        "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "gggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggggg",
    ] {
        assert!(RepairDigest::parse(invalid).is_err(), "accepted {invalid}");
    }
}

#[test]
fn durable_lint_scope_is_typed_and_matches_report_scope_kind() {
    let registered = RepairLintScope::registered("work".into()).unwrap();
    assert!(registered.matches_report_scope_kind(&LintScope::registered(
        LintOpaqueId::from_sorted_position(0).unwrap()
    )));
    assert!(!registered.matches_report_scope_kind(&LintScope::global()));
    assert!(RepairLintScope::registered(" ".into()).is_err());

    let value = serde_json::json!({"kind": "registered", "space": "work", "extra": true});
    assert!(serde_json::from_value::<RepairLintScope>(value).is_err());
}

#[test]
fn reclassification_rejects_noop_and_noncanonical_types() {
    assert!(RepairMutation::try_reclassify(Some("fact"), "fact").is_err());
    assert!(RepairMutation::try_reclassify(Some("fact"), "custom").is_err());

    let mutation = RepairMutation::try_reclassify(Some("fact"), "decision")
        .expect("supported reclassification");
    assert_eq!(mutation.before_memory_type(), Some("fact"));
    assert_eq!(mutation.after_memory_type(), "decision");
}

#[test]
fn mutation_deserializer_rejects_unknown_fields() {
    let value = serde_json::json!({
        "kind": "reclassify_memory",
        "before_memory_type": "fact",
        "after_memory_type": "decision",
        "hidden_target": "mem_other"
    });

    assert!(serde_json::from_value::<RepairMutation>(value).is_err());
}

#[test]
fn apply_request_binds_exact_manifest_digest() {
    let manifest_id = "repair_550e8400-e29b-41d4-a716-446655440000";
    let request = ApplyRepairRequest::try_new(
        manifest_id.into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        format!("apply repair {manifest_id} {SHA256_A}"),
    )
    .expect("valid apply request");

    assert_eq!(
        request.manifest_id(),
        "repair_550e8400-e29b-41d4-a716-446655440000"
    );
    assert_eq!(request.approved_manifest_digest().as_str(), SHA256_A);
    assert!(ApplyRepairRequest::try_new(
        manifest_id.into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        "yes apply it".into(),
    )
    .is_err());
}

#[test]
fn manifest_roundtrips_and_rejects_unsupported_writer() {
    let manifest = fixture_manifest();
    let value = serde_json::to_value(&manifest).unwrap();
    assert_eq!(value["manifest_schema_version"], 5);
    assert!(matches!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap(),
        StoredRepairManifest::V5(_)
    ));
    let roundtrip: RepairManifest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(roundtrip, manifest);

    let mut legacy_v4 = value.clone();
    legacy_v4["manifest_schema_version"] = serde_json::json!(4);
    legacy_v4["source"]["check_catalog_version"] = serde_json::json!(2);
    assert!(matches!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&legacy_v4).unwrap()).unwrap(),
        StoredRepairManifest::V4(_)
    ));

    let mut current_with_old_catalog = value.clone();
    current_with_old_catalog["source"]["check_catalog_version"] = serde_json::json!(2);
    assert!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&current_with_old_catalog).unwrap())
            .is_err(),
        "current manifests must not downgrade their lint check catalog"
    );

    // Catalog 3 -> 4 only added an advisory deep check: a manifest prepared
    // under 3 is still readable.
    let mut previous_catalog = value.clone();
    previous_catalog["source"]["check_catalog_version"] = serde_json::json!(3);
    assert!(matches!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&previous_catalog).unwrap()).unwrap(),
        StoredRepairManifest::V5(_)
    ));

    let mut unsupported = value;
    unsupported["writer"] = serde_json::json!("refresh_page");
    assert!(serde_json::from_value::<RepairManifest>(unsupported).is_err());
}

#[test]
fn manifest_rejects_noop_wrong_schema_and_unknown_fields() {
    let value = serde_json::to_value(fixture_manifest()).unwrap();

    let mut noop = value.clone();
    noop["mutation"]["after_memory_type"] = noop["mutation"]["before_memory_type"].clone();
    assert!(serde_json::from_value::<RepairManifest>(noop).is_err());

    let mut wrong_schema = value.clone();
    wrong_schema["manifest_schema_version"] = serde_json::json!(1);
    assert!(serde_json::from_value::<RepairManifest>(wrong_schema).is_err());

    let mut unknown = value;
    unknown["hidden_target"] = serde_json::json!("mem_other");
    assert!(serde_json::from_value::<RepairManifest>(unknown).is_err());
}

#[test]
fn apply_receipt_deserializer_revalidates_schema_and_effect_proof() {
    let target = RepairTarget::memory("mem_target".into(), RepairScope::uncategorized()).unwrap();
    let draft = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_001,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairAllowedEffects::memory_type(target),
        RepairWriter::ReclassifyMemory,
    )
    .unwrap();
    let canonical = draft.canonical_bytes().unwrap();
    assert!(!String::from_utf8(canonical)
        .unwrap()
        .contains("receipt_digest"));
    let receipt = RepairApplyReceipt::from_draft(draft, RepairDigest::parse(SHA256_B).unwrap());
    let value = serde_json::to_value(receipt).unwrap();
    assert_eq!(value["receipt_schema_version"], 4);
    assert!(matches!(
        StoredRepairApplyReceipt::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap(),
        StoredRepairApplyReceipt::V4(_)
    ));

    let mut wrong_schema = value.clone();
    wrong_schema["receipt_schema_version"] = serde_json::json!(1);
    assert!(serde_json::from_value::<RepairApplyReceipt>(wrong_schema).is_err());

    let mut escaped_effect = value;
    escaped_effect["non_target_after"] = serde_json::json!(SHA256_B);
    assert!(serde_json::from_value::<RepairApplyReceipt>(escaped_effect).is_err());
}

#[test]
fn apply_receipt_accepts_deterministic_writer_with_matching_effect() {
    let target = RepairTarget::memory("mem_target".into(), RepairScope::uncategorized()).unwrap();
    let draft = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_001,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairAllowedEffects::memory_source_agent(target),
        RepairWriter::NormalizeMemorySourceAgent,
    )
    .expect("matching deterministic receipt");

    let receipt = RepairApplyReceipt::from_draft(draft, RepairDigest::parse(SHA256_B).unwrap());
    let roundtrip: RepairApplyReceipt =
        serde_json::from_value(serde_json::to_value(receipt).unwrap()).unwrap();
    assert_eq!(roundtrip.writer(), RepairWriter::NormalizeMemorySourceAgent);
}

#[test]
fn verification_receipt_deserializer_rejects_unsupported_schema() {
    let draft = RepairVerificationReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        1_721_000_002,
        snapshots(7),
        snapshots(9),
    )
    .unwrap();
    let canonical: serde_json::Value =
        serde_json::from_slice(&draft.canonical_bytes().unwrap()).unwrap();
    assert!(canonical.get("receipt_digest").is_none());
    let receipt =
        RepairVerificationReceipt::from_draft(draft, RepairDigest::parse(SHA256_A).unwrap());
    let mut value = serde_json::to_value(receipt).unwrap();
    value["receipt_schema_version"] = serde_json::json!(5);

    assert!(serde_json::from_value::<RepairVerificationReceipt>(value).is_err());
}

#[test]
fn repair_v2_receipts_preserve_their_unsigned_canonical_bytes() {
    let target = RepairTarget::memory("mem_target".into(), RepairScope::uncategorized()).unwrap();
    let apply_draft = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_001,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairAllowedEffects::memory_type(target),
        RepairWriter::ReclassifyMemory,
    )
    .unwrap();
    let v2_apply_canonical = String::from_utf8(apply_draft.canonical_bytes().unwrap())
        .unwrap()
        .replacen(
            "\"receipt_schema_version\":4",
            "\"receipt_schema_version\":2",
            1,
        );
    let apply = RepairApplyReceipt::from_draft(apply_draft, RepairDigest::parse(SHA256_A).unwrap());
    let mut apply_value = serde_json::to_value(apply).unwrap();
    apply_value["receipt_schema_version"] = serde_json::json!(2);
    apply_value["receipt_digest"] =
        serde_json::json!(repair_test_sha256(v2_apply_canonical.as_bytes()));
    let stored_apply =
        StoredRepairApplyReceipt::from_slice(&serde_json::to_vec(&apply_value).unwrap()).unwrap();
    assert!(matches!(stored_apply, StoredRepairApplyReceipt::V2(_)));
    assert_eq!(
        repair_test_sha256(&stored_apply.canonical_unsigned_bytes().unwrap()),
        stored_apply.receipt_digest().as_str()
    );

    let verification_draft = RepairVerificationReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        1_721_000_002,
        snapshots(7),
        snapshots(9),
    )
    .unwrap();
    let v2_verification_canonical =
        String::from_utf8(verification_draft.canonical_bytes().unwrap())
            .unwrap()
            .replacen(
                "\"receipt_schema_version\":4",
                "\"receipt_schema_version\":2",
                1,
            );
    let verification = RepairVerificationReceipt::from_draft(
        verification_draft,
        RepairDigest::parse(SHA256_A).unwrap(),
    );
    let mut verification_value = serde_json::to_value(verification).unwrap();
    verification_value["receipt_schema_version"] = serde_json::json!(2);
    verification_value["receipt_digest"] =
        serde_json::json!(repair_test_sha256(v2_verification_canonical.as_bytes()));
    let stored_verification = StoredRepairVerificationReceipt::from_slice(
        &serde_json::to_vec(&verification_value).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        stored_verification,
        StoredRepairVerificationReceipt::V2(_)
    ));
    assert_eq!(
        repair_test_sha256(&stored_verification.canonical_unsigned_bytes().unwrap()),
        stored_verification.receipt_digest().as_str()
    );
    let current_verification = stored_verification
        .verify_and_try_into_current(|canonical, expected| {
            repair_test_sha256(canonical) == expected.as_str()
        })
        .unwrap();
    assert!(current_verification.deep_snapshots().is_some());
}

#[test]
fn general_only_deterministic_source_requires_paired_absent_deep_provenance() {
    let source = RepairSource::try_new_general_only_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        "identity.memory_state_integrity".into(),
        vec![],
        snapshots(1),
        LintProducerReceipt::new(None),
    )
    .unwrap();
    assert!(source.is_general_only_deterministic());
    assert!(source.deep_snapshots().is_none());
    assert!(source.deep_producer_receipt().is_none());

    let value = serde_json::to_value(&source).unwrap();
    assert!(value.get("deep_snapshots").is_none());
    assert!(value.get("deep_producer_receipt").is_none());
    let mut partial = value;
    partial["deep_snapshots"] = serde_json::to_value(snapshots(3)).unwrap();
    assert!(serde_json::from_value::<RepairSource>(partial).is_err());
}

#[test]
fn manifest_rejects_general_only_source_policy_mismatches() {
    let source = RepairSource::try_new_general_only_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        "identity.memory_state_integrity".into(),
        vec![],
        snapshots(1),
        LintProducerReceipt::new(None),
    )
    .unwrap();
    let target = RepairTarget::memory("mem_target".into(), RepairScope::uncategorized()).unwrap();
    let general = vec![RepairCheckBaseline::try_new_current(
        "identity.memory_state_integrity".into(),
        LintOutcome::Finding,
        LintGateEffect::Actionable,
        1,
        vec![],
    )
    .unwrap()];
    let assertions = RepairPostAssertions::try_new_general_only_for_check(
        "identity.memory_state_integrity".into(),
        LintDigest::from_u64(8),
        general.clone(),
        vec![],
    )
    .unwrap();
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        1_721_000_000,
        source,
        target.clone(),
        RepairExpectedState::try_new(None, RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::NormalizeMemorySourceAgent,
        RepairMutation::normalize_memory_source_agent(" ".into()).unwrap(),
        RepairAllowedEffects::memory_source_agent(target),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(GOLDEN_ROLLBACK_DIGEST).unwrap(),
        )
        .unwrap(),
        assertions,
    )
    .unwrap();
    let digest =
        RepairDigest::parse(&repair_test_sha256(&draft.canonical_bytes().unwrap())).unwrap();
    let manifest = RepairManifest::try_new(draft, digest).unwrap();
    let mut mismatched = serde_json::to_value(manifest).unwrap();
    mismatched["post_assertions"]["verification_policy"] = serde_json::json!({
        "kind": "applicable_checks",
        "required_deep_check_ids": []
    });
    assert!(serde_json::from_value::<RepairManifest>(mismatched).is_err());

    assert!(RepairPostAssertions::try_new_for_check(
        "identity.memory_state_integrity".into(),
        LintDigest::from_u64(8),
        general,
        vec![],
        vec![],
        vec![],
    )
    .is_err());
}

#[test]
fn verify_request_supports_general_only_and_deep_backed_shapes() {
    let producer = LintProducerReceipt::new(None);
    let general = complete_report(
        LintProfile::General,
        LintScope::global(),
        producer.clone(),
        1,
    );
    let general_only = VerifyRepairRequest::try_new_general_only(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        general.clone(),
    )
    .unwrap();
    assert!(general_only.deep_report().is_none());
    let roundtrip: VerifyRepairRequest =
        serde_json::from_value(serde_json::to_value(general_only).unwrap()).unwrap();
    assert!(roundtrip.deep_report().is_none());

    let deep = complete_report(LintProfile::Deep, LintScope::global(), producer, 3);
    let deep_backed = VerifyRepairRequest::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        general.clone(),
        deep,
    )
    .unwrap();
    assert!(deep_backed.deep_report().is_some());

    let mismatched = complete_report(
        LintProfile::Deep,
        LintScope::global(),
        LintProducerReceipt::new(Some(
            crate::lint::LintCommitReceipt::new("0123456789abcdef0123456789abcdef01234567")
                .unwrap(),
        )),
        5,
    );
    assert!(VerifyRepairRequest::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        general,
        mismatched,
    )
    .is_err());
}

#[test]
fn orphan_revision_manifest_is_exactly_pending_revision_only() {
    let source = RepairSource::try_new_general_only_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        "identity.memory_state_integrity".into(),
        vec![],
        snapshots(1),
        LintProducerReceipt::new(None),
    )
    .unwrap();
    let target = RepairTarget::memory(
        "mem_orphan_revision".into(),
        RepairScope::registered("wenlan".into()).unwrap(),
    )
    .unwrap();
    let assertions = RepairPostAssertions::try_new_general_only_for_check(
        "identity.memory_state_integrity".into(),
        LintDigest::from_u64(8),
        vec![RepairCheckBaseline::try_new_current(
            "identity.memory_state_integrity".into(),
            LintOutcome::Finding,
            LintGateEffect::Actionable,
            1,
            vec![],
        )
        .unwrap()],
        vec!["memories.supersession_integrity".into()],
    )
    .unwrap();
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440201".into(),
        1_721_000_000,
        source,
        target.clone(),
        RepairExpectedState::try_new(Some(1), RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::UnstageOrphanRevision,
        RepairMutation::unstage_orphan_revision(),
        RepairAllowedEffects::memory_pending_revision(target),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(SHA256_B).unwrap(),
        )
        .unwrap(),
        assertions,
    )
    .expect("orphan revision repair is a valid exact manifest");
    let manifest = RepairManifest::try_new(draft, RepairDigest::parse(SHA256_A).unwrap()).unwrap();
    let value = serde_json::to_value(&manifest).unwrap();
    assert_eq!(value["manifest_schema_version"], 5);
    assert_eq!(value["writer"], "unstage_orphan_revision");
    assert_eq!(value["mutation"]["kind"], "unstage_orphan_revision");
    assert_eq!(
        value["allowed_effects"]["fields"],
        serde_json::json!(["pending_revision"])
    );
    let roundtrip: RepairManifest = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(roundtrip.writer(), RepairWriter::UnstageOrphanRevision);
    assert!(matches!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap(),
        StoredRepairManifest::V5(_)
    ));

    let mut escaped = value;
    escaped["allowed_effects"]["fields"] = serde_json::json!(["supersedes"]);
    assert!(
        serde_json::from_value::<RepairManifest>(escaped).is_err(),
        "orphan revision repair must not acquire supersedes authority"
    );

    let mut legacy_schema = serde_json::to_value(&manifest).unwrap();
    legacy_schema["manifest_schema_version"] = serde_json::json!(4);
    assert!(
        serde_json::from_value::<RepairManifest>(legacy_schema).is_err(),
        "v4 readers must not accept v5 writers"
    );
}

#[test]
fn orphan_memory_entity_link_contract_names_the_exact_pair() {
    let target = RepairTarget::memory_entity_link(
        "mem_missing".into(),
        "entity_present".into(),
        RepairScope::global(),
    )
    .unwrap();
    let target_value = serde_json::to_value(&target).unwrap();
    assert_eq!(target_value["kind"], "memory_entity_link");
    assert_eq!(target_value["memory_id"], "mem_missing");
    assert_eq!(target_value["entity_id"], "entity_present");
    assert_eq!(
        serde_json::from_value::<RepairTarget>(target_value).unwrap(),
        target
    );

    let mutation =
        RepairMutation::delete_memory_entity_link("mem_missing", "entity_present").unwrap();
    let mutation_value = serde_json::to_value(&mutation).unwrap();
    assert_eq!(mutation_value["kind"], "delete_memory_entity_link");
    assert_eq!(
        serde_json::from_value::<RepairMutation>(mutation_value).unwrap(),
        mutation
    );

    let effects = RepairAllowedEffects::memory_entity_link(target.clone());
    let receipt = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440202".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_001,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        effects,
        RepairWriter::DeleteMemoryEntityLink,
    )
    .expect("the exact pair is a valid receipt owner");
    let receipt = RepairApplyReceipt::from_draft(receipt, RepairDigest::parse(SHA256_B).unwrap());
    assert_eq!(receipt.writer(), RepairWriter::DeleteMemoryEntityLink);
    let receipt_value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(receipt_value["receipt_schema_version"], 4);
    assert!(matches!(
        StoredRepairApplyReceipt::from_slice(&serde_json::to_vec(&receipt_value).unwrap()).unwrap(),
        StoredRepairApplyReceipt::V4(_)
    ));
    let mut legacy_receipt = receipt_value;
    legacy_receipt["receipt_schema_version"] = serde_json::json!(3);
    assert!(
        serde_json::from_value::<RepairApplyReceipt>(legacy_receipt).is_err(),
        "v3 receipts must not claim a v4 writer"
    );

    assert!(RepairTarget::memory_entity_link(
        "".into(),
        "entity_present".into(),
        RepairScope::global(),
    )
    .is_err());
    assert!(RepairTarget::memory_entity_link(
        "mem_missing".into(),
        "".into(),
        RepairScope::global(),
    )
    .is_err());
}

#[test]
fn v4_verification_receipt_allows_absent_deep_and_preserves_v3_reader() {
    let draft = RepairVerificationReceiptDraft::try_new_general_only(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        1_721_000_002,
        snapshots(7),
    )
    .unwrap();
    let receipt =
        RepairVerificationReceipt::from_draft(draft, RepairDigest::parse(SHA256_A).unwrap());
    assert!(receipt.deep_snapshots().is_none());
    let value = serde_json::to_value(&receipt).unwrap();
    assert_eq!(value["receipt_schema_version"], 4);
    assert!(value.get("deep_snapshots").is_none());
    assert!(serde_json::from_value::<RepairVerificationReceipt>(value.clone()).is_ok());
    assert!(matches!(
        StoredRepairVerificationReceipt::from_slice(&serde_json::to_vec(&value).unwrap()).unwrap(),
        StoredRepairVerificationReceipt::V4(_)
    ));

    let mut legacy_v3 = value.clone();
    legacy_v3["receipt_schema_version"] = serde_json::json!(3);
    assert!(serde_json::from_value::<RepairVerificationReceipt>(legacy_v3.clone()).is_ok());
    assert!(matches!(
        StoredRepairVerificationReceipt::from_slice(&serde_json::to_vec(&legacy_v3).unwrap())
            .unwrap(),
        StoredRepairVerificationReceipt::V3(_)
    ));

    let mut legacy_v2 = value;
    legacy_v2["receipt_schema_version"] = serde_json::json!(2);
    assert!(serde_json::from_value::<RepairVerificationReceipt>(legacy_v2).is_err());

    let direct = RepairVerificationReceipt::try_new_general_only(
        "repair_550e8400-e29b-41d4-a716-446655440000".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        1_721_000_002,
        snapshots(7),
        RepairDigest::parse(SHA256_A).unwrap(),
    )
    .unwrap();
    assert!(direct.deep_snapshots().is_none());
}

#[test]
fn empty_source_page_archive_contract_is_status_only_in_v5_v4_receipts() {
    let source = RepairSource::try_new_general_only_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        "pages.source_page_integrity".into(),
        vec![],
        snapshots(1),
        LintProducerReceipt::new(None),
    )
    .unwrap();
    let target =
        RepairTarget::page("page_empty_source".into(), RepairScope::uncategorized()).unwrap();
    let assertions = RepairPostAssertions::try_new_general_only_for_check(
        "pages.source_page_integrity".into(),
        LintDigest::from_u64(9),
        vec![RepairCheckBaseline::try_new_current(
            "pages.source_page_integrity".into(),
            LintOutcome::Finding,
            LintGateEffect::Actionable,
            1,
            vec![],
        )
        .unwrap()],
        vec!["pages.projection.identity".into()],
    )
    .unwrap();
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440203".into(),
        1_721_000_003,
        source,
        target.clone(),
        RepairExpectedState::try_new(Some(1), RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::ArchiveEmptySourcePage,
        RepairMutation::archive_empty_source_page(),
        RepairAllowedEffects::page_status(target.clone()),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(SHA256_B).unwrap(),
        )
        .unwrap(),
        assertions,
    )
    .expect("an empty source Page can be archived with status-only authority");
    let manifest = RepairManifest::try_new(draft, RepairDigest::parse(SHA256_A).unwrap()).unwrap();
    let value = serde_json::to_value(&manifest).unwrap();
    assert_eq!(value["manifest_schema_version"], 5);
    assert_eq!(value["target"]["kind"], "page");
    assert_eq!(value["writer"], "archive_empty_source_page");
    assert_eq!(value["mutation"]["kind"], "archive_empty_source_page");
    assert_eq!(value["mutation"]["before_status"], "active");
    assert_eq!(value["mutation"]["after_status"], "archived");
    assert_eq!(
        value["allowed_effects"]["fields"],
        serde_json::json!(["page_status"])
    );
    assert_eq!(
        serde_json::from_value::<RepairManifest>(value.clone())
            .unwrap()
            .writer(),
        RepairWriter::ArchiveEmptySourcePage
    );
    let mut legacy_manifest = value;
    legacy_manifest["manifest_schema_version"] = serde_json::json!(4);
    assert!(serde_json::from_value::<RepairManifest>(legacy_manifest).is_err());

    let receipt = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440203".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_004,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairAllowedEffects::page_status(target),
        RepairWriter::ArchiveEmptySourcePage,
    )
    .unwrap();
    let receipt = RepairApplyReceipt::from_draft(receipt, RepairDigest::parse(SHA256_B).unwrap());
    let receipt_value = serde_json::to_value(receipt).unwrap();
    assert_eq!(receipt_value["receipt_schema_version"], 4);
    let mut legacy_receipt = receipt_value;
    legacy_receipt["receipt_schema_version"] = serde_json::json!(3);
    assert!(serde_json::from_value::<RepairApplyReceipt>(legacy_receipt).is_err());
}

#[test]
fn stale_projection_quarantine_contract_is_v5_v4_only() {
    let source = RepairSource::try_new_general_only_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        "pages.projection.identity".into(),
        vec![],
        snapshots(1),
        LintProducerReceipt::new(None),
    )
    .unwrap();
    let target = RepairTarget::page_projection("page_stale".into(), RepairScope::global()).unwrap();
    let assertions = RepairPostAssertions::try_new_general_only_for_check(
        "pages.projection.identity".into(),
        LintDigest::from_u64(10),
        vec![RepairCheckBaseline::try_new_current(
            "pages.projection.identity".into(),
            LintOutcome::Finding,
            LintGateEffect::Actionable,
            1,
            vec![],
        )
        .unwrap()],
        vec![],
    )
    .unwrap();
    let draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440204".into(),
        1_721_000_003,
        source,
        target.clone(),
        RepairExpectedState::try_new(None, RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::QuarantineStalePageProjection,
        RepairMutation::quarantine_stale_page_projection(
            "stale.md".into(),
            ".wenlan/orphaned/page_stale.md".into(),
        )
        .unwrap(),
        RepairAllowedEffects::page_projection_quarantine(target.clone()),
        RepairRollbackArtifact::try_new(
            "rollback-v1.json".into(),
            RepairDigest::parse(SHA256_B).unwrap(),
        )
        .unwrap(),
        assertions,
    )
    .unwrap();
    let manifest = RepairManifest::try_new(draft, RepairDigest::parse(SHA256_A).unwrap()).unwrap();
    let value = serde_json::to_value(&manifest).unwrap();
    assert_eq!(value["manifest_schema_version"], 5);
    assert_eq!(value["writer"], "quarantine_stale_page_projection");
    assert_eq!(
        value["allowed_effects"]["fields"],
        serde_json::json!(["page_projection_quarantine"])
    );
    let mut legacy_manifest = value;
    legacy_manifest["manifest_schema_version"] = serde_json::json!(4);
    assert!(serde_json::from_value::<RepairManifest>(legacy_manifest).is_err());

    let receipt = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440204".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_004,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairAllowedEffects::page_projection_quarantine(target),
        RepairWriter::QuarantineStalePageProjection,
    )
    .unwrap();
    let receipt = RepairApplyReceipt::from_draft(receipt, RepairDigest::parse(SHA256_B).unwrap());
    let receipt_value = serde_json::to_value(receipt).unwrap();
    assert_eq!(receipt_value["receipt_schema_version"], 4);
    let mut legacy_receipt = receipt_value;
    legacy_receipt["receipt_schema_version"] = serde_json::json!(3);
    assert!(serde_json::from_value::<RepairApplyReceipt>(legacy_receipt).is_err());
}

fn complete_deep_report(scope: LintScope, producer_receipt: LintProducerReceipt) -> LintReport {
    let base = complete_report(
        LintProfile::Deep,
        scope.clone(),
        producer_receipt.clone(),
        30,
    );
    let populations = LintSemanticCheckId::ALL
        .into_iter()
        .map(|check_id| LintSemanticPopulation::try_new(check_id, 0, 0, 0, false).unwrap())
        .collect();
    let work =
        LintAgentWork::try_new(LintDigest::from_u64(31), populations, vec![], vec![]).unwrap();
    LintReport::try_new_for_profile_with_agent_work(
        LintProfile::Deep,
        scope,
        LintCapabilityContext::daemon_operator_endpoint(),
        snapshots(30),
        LintConfigFingerprint::from_effective_config(&[]),
        producer_receipt,
        base.checks().to_vec(),
        Some(work),
    )
    .unwrap()
}

fn reclassification_finding() -> LintSemanticFinding {
    LintSemanticFinding::try_new(
        LintOpaqueId::from_sorted_position(0).unwrap(),
        LintSemanticAction::ReclassifyMemory,
        LintSemanticReasonCode::ClassificationMismatch,
        9_000,
        LintSemanticProviderRoute::CallingAgent,
        vec![LintDigest::from_u64(32)],
        vec![],
    )
    .unwrap()
}

fn lowercase_embedding_hex() -> String {
    (0..768)
        .flat_map(|index| ((index as f32) / 768.0).to_le_bytes())
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[test]
fn prepare_request_normalizes_tagged_and_legacy_choices_and_fails_closed() {
    let producer = LintProducerReceipt::new(None);
    let general = complete_report(
        LintProfile::General,
        LintScope::global(),
        producer.clone(),
        20,
    );
    let rename = RepairChoice::rename_page_title(
        "lint_review_page_title".into(),
        "page_exact".into(),
        "Before".into(),
        "After".into(),
    )
    .unwrap();
    let tagged = PrepareRepairRequest::try_new_with_choice(
        RepairLintScope::global(),
        general.clone(),
        None,
        rename.clone(),
    )
    .expect("deterministic review choices do not require Deep");
    assert_eq!(tagged.choice(), &rename);
    assert!(tagged.deep_report().is_none());

    let finding = reclassification_finding();
    let deep = complete_deep_report(LintScope::global(), producer);
    let reclass = PrepareRepairRequest::try_new_with_choice(
        RepairLintScope::global(),
        general,
        Some(deep),
        RepairChoice::reclassify_memory(finding.clone(), MemoryType::Decision).unwrap(),
    )
    .unwrap();
    let mut legacy = serde_json::to_value(reclass).unwrap();
    let choice = legacy
        .as_object_mut()
        .unwrap()
        .remove("choice")
        .expect("tagged choice");
    legacy["selected_finding"] = choice["selected_finding"].clone();
    legacy["after_memory_type"] = choice["after_memory_type"].clone();
    let normalized: PrepareRepairRequest =
        serde_json::from_value(legacy.clone()).expect("one-release legacy request");
    assert!(matches!(
        normalized.choice(),
        RepairChoice::ReclassifyMemory { .. }
    ));

    let mut mixed = legacy.clone();
    mixed["choice"] = choice.clone();
    assert!(serde_json::from_value::<PrepareRepairRequest>(mixed).is_err());
    let mut partial = legacy.clone();
    partial.as_object_mut().unwrap().remove("after_memory_type");
    assert!(serde_json::from_value::<PrepareRepairRequest>(partial).is_err());
    let mut neither = legacy.clone();
    neither.as_object_mut().unwrap().remove("selected_finding");
    neither.as_object_mut().unwrap().remove("after_memory_type");
    assert!(serde_json::from_value::<PrepareRepairRequest>(neither).is_err());

    let mut tagged_with_legacy = serde_json::to_value(tagged).unwrap();
    tagged_with_legacy["after_memory_type"] = serde_json::json!("decision");
    assert!(serde_json::from_value::<PrepareRepairRequest>(tagged_with_legacy).is_err());

    let mut tagged_with_null_legacy = serde_json::to_value(
        PrepareRepairRequest::try_new_with_choice(
            RepairLintScope::global(),
            complete_report(
                LintProfile::General,
                LintScope::global(),
                LintProducerReceipt::new(None),
                20,
            ),
            None,
            rename,
        )
        .unwrap(),
    )
    .unwrap();
    tagged_with_null_legacy["selected_finding"] = serde_json::Value::Null;
    tagged_with_null_legacy["after_memory_type"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<PrepareRepairRequest>(tagged_with_null_legacy).is_err());

    let mut legacy_with_null_choice = legacy;
    legacy_with_null_choice["choice"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<PrepareRepairRequest>(legacy_with_null_choice).is_err());
}

#[test]
fn review_choices_validate_exact_canonical_payloads() {
    assert!(RepairChoice::rename_page_title(
        "review".into(),
        "page_exact".into(),
        "Before".into(),
        "After".into(),
    )
    .is_ok());
    for invalid in [
        RepairChoice::rename_page_title(
            " ".into(),
            "page_exact".into(),
            "Before".into(),
            "After".into(),
        ),
        RepairChoice::rename_page_title(
            "review".into(),
            " page_exact".into(),
            "Before".into(),
            "After".into(),
        ),
        RepairChoice::rename_page_title(
            "review".into(),
            "page_exact".into(),
            "Before".into(),
            "Before".into(),
        ),
        RepairChoice::rename_page_title(
            "review".into(),
            "page_exact".into(),
            " Before".into(),
            "After".into(),
        ),
    ] {
        assert!(invalid.is_err());
    }

    assert!(RepairChoice::complete_entity_extraction(
        "review".into(),
        "mem_exact".into(),
        vec!["entity_a".into(), "entity_b".into()],
    )
    .is_ok());
    assert!(
        RepairChoice::complete_entity_extraction("review".into(), "mem_exact".into(), vec![])
            .is_ok()
    );
    for ids in [
        vec!["entity_b".into(), "entity_a".into()],
        vec!["entity_a".into(), "entity_a".into()],
        vec![" ".into()],
    ] {
        assert!(
            RepairChoice::complete_entity_extraction("review".into(), "mem_exact".into(), ids,)
                .is_err()
        );
    }
}

#[test]
fn empty_entity_extraction_target_mutation_and_choice_roundtrip() {
    let target = RepairTarget::memory_entity_extraction(
        "mem_exact".into(),
        RepairEnrichmentStep::EntityExtract,
        vec![],
        RepairScope::uncategorized(),
    )
    .unwrap();
    let target_roundtrip: RepairTarget =
        serde_json::from_value(serde_json::to_value(&target).unwrap()).unwrap();
    assert_eq!(target_roundtrip, target);

    let mutation = RepairMutation::complete_entity_extraction(vec![]).unwrap();
    let mutation_roundtrip: RepairMutation =
        serde_json::from_value(serde_json::to_value(&mutation).unwrap()).unwrap();
    assert_eq!(mutation_roundtrip, mutation);

    let choice =
        RepairChoice::complete_entity_extraction("review".into(), "mem_exact".into(), vec![])
            .unwrap();
    let choice_roundtrip: RepairChoice =
        serde_json::from_value(serde_json::to_value(&choice).unwrap()).unwrap();
    assert_eq!(choice_roundtrip, choice);
}

#[test]
fn entity_extraction_rejects_blank_duplicate_and_unsorted_ids() {
    let target = RepairTarget::memory_entity_extraction(
        "mem_exact".to_owned(),
        RepairEnrichmentStep::EntityExtract,
        vec![],
        RepairScope::uncategorized(),
    )
    .unwrap();
    let mutation = RepairMutation::complete_entity_extraction(vec![]).unwrap();
    let choice = RepairChoice::complete_entity_extraction(
        "review".to_owned(),
        "mem_exact".to_owned(),
        vec![],
    )
    .unwrap();
    let target_value = serde_json::to_value(target).unwrap();
    let mutation_value = serde_json::to_value(mutation).unwrap();
    let choice_value = serde_json::to_value(choice).unwrap();

    for ids in [
        vec![" ".to_owned()],
        vec!["entity_a".to_owned(), "entity_a".to_owned()],
        vec!["entity_b".to_owned(), "entity_a".to_owned()],
    ] {
        let mut target_value = target_value.clone();
        target_value["entity_ids"] = serde_json::to_value(&ids).unwrap();
        assert!(serde_json::from_value::<RepairTarget>(target_value).is_err());

        let mut mutation_value = mutation_value.clone();
        mutation_value["entity_ids"] = serde_json::to_value(&ids).unwrap();
        assert!(serde_json::from_value::<RepairMutation>(mutation_value).is_err());

        let mut choice_value = choice_value.clone();
        choice_value["entity_ids"] = serde_json::to_value(&ids).unwrap();
        assert!(serde_json::from_value::<RepairChoice>(choice_value).is_err());
    }
}

fn review_source(check_id: &str, review_id: &str, owner_ids: Vec<String>) -> RepairSource {
    RepairSource::try_new_general_only_deterministic(
        RepairLintScope::global(),
        LintScope::global(),
        check_id.into(),
        vec![],
        snapshots(40),
        LintProducerReceipt::new(None),
    )
    .unwrap()
    .try_with_review_binding(
        RepairReviewBinding::try_new(
            review_id.into(),
            RepairDigest::parse(SHA256_A).unwrap(),
            owner_ids,
        )
        .unwrap(),
    )
    .unwrap()
}

fn general_assertions(check_id: &str) -> RepairPostAssertions {
    RepairPostAssertions::try_new_general_only_for_check(
        check_id.into(),
        LintDigest::from_u64(41),
        vec![RepairCheckBaseline::try_new_current(
            check_id.into(),
            LintOutcome::Finding,
            LintGateEffect::Actionable,
            1,
            vec![],
        )
        .unwrap()],
        vec![],
    )
    .unwrap()
}

#[test]
fn aggregate_contracts_are_v6_manifest_and_v5_receipt_only() {
    let page_id = "page_exact";
    let page_target = RepairTarget::page_projection(page_id.into(), RepairScope::global()).unwrap();
    let page_draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440301".into(),
        1_721_000_100,
        review_source(
            "pages.duplicate_active_titles",
            "lint_review_page_title",
            vec![page_id.into()],
        ),
        page_target.clone(),
        RepairExpectedState::try_new(Some(4), RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::RenamePageTitle,
        RepairMutation::rename_page_title(
            "Before".into(),
            "After".into(),
            lowercase_embedding_hex(),
        )
        .unwrap(),
        RepairAllowedEffects::page_title_rename(page_target.clone()),
        RepairRollbackArtifact::try_new_v2(
            "rollback-v2.json".into(),
            RepairDigest::parse(SHA256_B).unwrap(),
        )
        .unwrap(),
        general_assertions("pages.duplicate_active_titles"),
    )
    .unwrap();
    let page_manifest =
        RepairManifest::try_new(page_draft, RepairDigest::parse(SHA256_A).unwrap()).unwrap();
    let page_value = serde_json::to_value(&page_manifest).unwrap();
    assert_eq!(page_value["manifest_schema_version"], 6);
    assert_eq!(
        page_value["allowed_effects"]["fields"],
        serde_json::json!([
            "page_title",
            "page_version",
            "page_embedding",
            "page_projection"
        ])
    );
    assert!(matches!(
        StoredRepairManifest::from_slice(&serde_json::to_vec(&page_value).unwrap()).unwrap(),
        StoredRepairManifest::V6(_)
    ));
    let mut legacy_page = page_value;
    legacy_page["manifest_schema_version"] = serde_json::json!(5);
    assert!(serde_json::from_value::<RepairManifest>(legacy_page).is_err());

    let entity_ids = vec!["entity_a".to_string(), "entity_b".to_string()];
    let entity_target = RepairTarget::memory_entity_extraction(
        "mem_exact".into(),
        RepairEnrichmentStep::EntityExtract,
        entity_ids.clone(),
        RepairScope::uncategorized(),
    )
    .unwrap();
    let entity_draft = RepairManifestDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440302".into(),
        1_721_000_101,
        review_source(
            "memories.enrichment_failures",
            "lint_review_entity_extract",
            vec!["mem_exact".into()],
        ),
        entity_target.clone(),
        RepairExpectedState::try_new(None, RepairDigest::parse(SHA256_A).unwrap()).unwrap(),
        RepairWriter::CompleteEntityExtraction,
        RepairMutation::complete_entity_extraction(entity_ids).unwrap(),
        RepairAllowedEffects::complete_entity_extraction(entity_target.clone()),
        RepairRollbackArtifact::try_new_v2(
            "rollback-v2.json".into(),
            RepairDigest::parse(SHA256_B).unwrap(),
        )
        .unwrap(),
        general_assertions("memories.enrichment_failures"),
    )
    .unwrap();
    let entity_manifest =
        RepairManifest::try_new(entity_draft, RepairDigest::parse(SHA256_A).unwrap()).unwrap();
    let entity_value = serde_json::to_value(entity_manifest).unwrap();
    assert_eq!(
        entity_value["allowed_effects"]["fields"],
        serde_json::json!([
            "memory_entity_links",
            "enrichment_step",
            "entity_establishment"
        ])
    );
    // A manifest prepared before #708 declared only the two link/step
    // effects. It is still on disk waiting to be applied, so it must stay
    // valid; only newly prepared manifests name the establishment write.
    let mut pre_708_entity = entity_value.clone();
    pre_708_entity["allowed_effects"]["fields"] =
        serde_json::json!(["memory_entity_links", "enrichment_step"]);
    assert!(serde_json::from_value::<RepairManifest>(pre_708_entity).is_ok());

    let mut legacy_entity = entity_value;
    legacy_entity["manifest_schema_version"] = serde_json::json!(5);
    assert!(serde_json::from_value::<RepairManifest>(legacy_entity).is_err());

    let receipt = RepairApplyReceiptDraft::try_new(
        "repair_550e8400-e29b-41d4-a716-446655440301".into(),
        RepairDigest::parse(SHA256_A).unwrap(),
        1_721_000_102,
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_A).unwrap(),
        RepairDigest::parse(SHA256_B).unwrap(),
        RepairAllowedEffects::page_title_rename(page_target),
        RepairWriter::RenamePageTitle,
    )
    .unwrap();
    let receipt = RepairApplyReceipt::from_draft(receipt, RepairDigest::parse(SHA256_B).unwrap());
    let receipt_value = serde_json::to_value(receipt).unwrap();
    assert_eq!(receipt_value["receipt_schema_version"], 5);
    assert!(matches!(
        StoredRepairApplyReceipt::from_slice(&serde_json::to_vec(&receipt_value).unwrap()).unwrap(),
        StoredRepairApplyReceipt::V5(_)
    ));
    let mut legacy_receipt = receipt_value;
    legacy_receipt["receipt_schema_version"] = serde_json::json!(4);
    assert!(serde_json::from_value::<RepairApplyReceipt>(legacy_receipt).is_err());
}

#[test]
fn typed_rollback_v2_roundtrips_and_frozen_v1_stays_unchanged() {
    let single = RepairRollbackV2::try_new(
        RepairRollbackPayloadV2::single_table(
            "memories".into(),
            "mem_exact".into(),
            vec!["source_id".into()],
            vec![vec!["'mem_exact'".into()]],
        )
        .unwrap(),
    )
    .unwrap();
    let page = RepairRollbackV2::try_new(
        RepairRollbackPayloadV2::rename_page_title(
            "page_exact".into(),
            vec!["id".into(), "embedding_hex".into()],
            vec!["'page_exact'".into(), "00".repeat(768 * 4)],
            "page-exact.md".into(),
            vec![
                RepairRollbackFileEntry::file(".wenlan/state.json".into(), b"{}".to_vec()).unwrap(),
                RepairRollbackFileEntry::file("page-exact.md".into(), b"before page".to_vec())
                    .unwrap(),
            ],
        )
        .unwrap(),
    )
    .unwrap();

    assert!(RepairRollbackPayloadV2::rename_page_title(
        "page_exact".into(),
        vec!["id".into()],
        vec!["'page_exact'".into()],
        "page-exact.md".into(),
        vec![RepairRollbackFileEntry::file(".wenlan/state.json".into(), b"{}".to_vec()).unwrap(),],
    )
    .is_err());
    assert!(RepairRollbackPayloadV2::rename_page_title(
        "page_exact".into(),
        vec!["id".into()],
        vec!["'page_exact'".into()],
        "page-exact.md".into(),
        vec![
            RepairRollbackFileEntry::file(".wenlan/state.json".into(), b"{}".to_vec()).unwrap(),
            RepairRollbackFileEntry::file("other.md".into(), b"unrelated".to_vec()).unwrap(),
        ],
    )
    .is_err());
    let entity = RepairRollbackV2::try_new(
        RepairRollbackPayloadV2::complete_entity_extraction(
            "mem_exact".into(),
            vec!["id".into()],
            vec!["'mem_exact'".into()],
            vec!["entity_old".into()],
            "failed".into(),
            Some("transient".into()),
            2,
            1_721_000_000,
        )
        .unwrap(),
    )
    .unwrap();

    for rollback in [single, page, entity] {
        let bytes = serde_json::to_vec(&rollback).unwrap();
        assert!(matches!(
            StoredRepairRollbackArtifact::from_slice(&bytes).unwrap(),
            StoredRepairRollbackArtifact::V2(_)
        ));
        let decoded: RepairRollbackV2 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(decoded, rollback);
    }

    let fixture = persisted_fixture(include_str!("../testdata/repair/v1/rollback.json"));
    let frozen =
        StoredRepairRollbackArtifact::from_slice(fixture).expect("frozen v1 remains readable");
    assert!(matches!(frozen, StoredRepairRollbackArtifact::V1(_)));
    assert_eq!(frozen.persisted_bytes().unwrap(), fixture);
}

#[test]
fn entity_extraction_rollback_preserves_whitespace_and_empty_error_text_exactly() {
    for expected_error in ["", "  transient  ", "\tline one\nline two "] {
        let rollback = RepairRollbackV2::try_new(
            RepairRollbackPayloadV2::complete_entity_extraction(
                "mem_exact".into(),
                vec!["id".into()],
                vec!["t:6d656d5f6578616374".into()],
                vec![],
                "failed".into(),
                Some(expected_error.to_string()),
                1,
                1_721_000_000,
            )
            .unwrap(),
        )
        .unwrap();
        let bytes = serde_json::to_vec(&rollback).unwrap();
        let decoded: RepairRollbackV2 = serde_json::from_slice(&bytes).unwrap();
        let RepairRollbackPayloadV2::CompleteEntityExtraction {
            enrichment_error, ..
        } = decoded.payload()
        else {
            panic!("expected entity extraction rollback");
        };

        assert_eq!(enrichment_error.as_deref(), Some(expected_error));
    }
}
