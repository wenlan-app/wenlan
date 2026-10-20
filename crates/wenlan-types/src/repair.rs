// SPDX-License-Identifier: Apache-2.0
//! Approval-gated repair contracts shared by the daemon and local clients.

mod frozen_v1;

use crate::repair_relation::{
    EntityRelationRepairChoice, EntityRelationRepairSelection, RepairRelationMutation,
    RepairRelationSnapshot,
};
use crate::{
    lint::{
        LintCommitReceipt, LintDbSnapshotMode, LintDbSnapshotReceipt, LintDigest, LintEvidenceRef,
        LintGateEffect, LintOpaqueId, LintOutcome, LintPageSnapshotMode, LintPageSnapshotReceipt,
        LintProducerReceipt, LintProfile, LintReasonCode, LintSafeRootRelativePath, LintScope,
        LintScopeKind, LintSemanticAction, LintSemanticFinding, LintSemanticProviderRoute,
        LintSemanticReasonCode, LintSnapshotReceipts, LINT_CHECK_CATALOG_VERSION,
        LINT_REPORT_SCHEMA_VERSION,
    },
    LintReport, MemoryType,
};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use std::{fmt, path::Component, path::Path};

use frozen_v1::{
    FrozenRepairApplyReceiptV1, FrozenRepairManifestPreBaselineV1, FrozenRepairManifestV1,
    FrozenRepairRollbackArtifactV1, FrozenRepairVerificationReceiptV1,
};

pub const REPAIR_MANIFEST_SCHEMA_VERSION: u16 = 7;
pub const REPAIR_ROLLBACK_FORMAT_VERSION: u16 = 2;
pub const REPAIR_RELATION_ROLLBACK_FORMAT_VERSION: u16 = 3;
pub const REPAIR_RECEIPT_SCHEMA_VERSION: u16 = 5;
pub const REPAIR_RELATION_RECEIPT_SCHEMA_VERSION: u16 = 6;
const PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION: u16 = 6;
const LEGACY_REPAIR_MANIFEST_SCHEMA_VERSION: u16 = 5;
const PREVIOUS_REPAIR_ROLLBACK_FORMAT_VERSION: u16 = 1;
const PREVIOUS_REPAIR_RECEIPT_SCHEMA_VERSION: u16 = 4;
const REPAIR_VERIFICATION_RECEIPT_SCHEMA_VERSION: u16 = 4;
pub const REPAIR_CLASSIFICATION_CHECK_ID: &str = "memories.semantic.classification";
pub const REPAIR_RELATION_CHECK_ID: &str = "kg.semantic.entity_relations";
const PREVIOUS_LINT_REPORT_SCHEMA_VERSION: u16 = 4;
/// Catalog 3 -> 4 only added an advisory deep check, so a manifest prepared
/// under 3 stays readable and applicable.
const PREVIOUS_LINT_CHECK_CATALOG_VERSION: u16 = 3;
/// The catalog of manifests written before report schema 5 / manifest schema 5;
/// readable only inside the older manifest schemas.
const LEGACY_LINT_CHECK_CATALOG_VERSION: u16 = 2;
const REPAIR_MEMORY_STATE_CHECK_ID: &str = "identity.memory_state_integrity";
const REPAIR_TAG_INTEGRITY_CHECK_ID: &str = "identity.tag_integrity";
const REPAIR_SUPERSESSION_CHECK_ID: &str = "memories.supersession_integrity";
const REPAIR_MEMORY_ENTITY_INTEGRITY_CHECK_ID: &str = "memory_entities.integrity";
const REPAIR_ORPHAN_LABELS_CHECK_ID: &str = "pages.links.orphan_labels";
const REPAIR_PROJECTION_IDENTITY_CHECK_ID: &str = "pages.projection.identity";
const REPAIR_PROJECTION_VERSION_CHECK_ID: &str = "pages.projection.version_alignment";
const REPAIR_SOURCE_PAGE_INTEGRITY_CHECK_ID: &str = "pages.source_page_integrity";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StoredRepairDigestRef<'a>(&'a str);

impl<'a> StoredRepairDigestRef<'a> {
    pub const fn as_str(self) -> &'a str {
        self.0
    }
}

#[derive(Deserialize)]
struct StoredManifestVersionProbe {
    manifest_schema_version: u16,
}

#[derive(Deserialize)]
struct StoredRollbackVersionProbe {
    format_version: u16,
}

#[derive(Deserialize)]
struct StoredReceiptVersionProbe {
    receipt_schema_version: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredRepairManifest {
    V1(FrozenRepairManifestV1),
    V1PreBaseline(FrozenRepairManifestPreBaselineV1),
    V2(Box<RepairManifest>),
    V3(Box<RepairManifest>),
    V4(Box<RepairManifest>),
    V5(Box<RepairManifest>),
    V6(Box<RepairManifest>),
    V7(Box<RepairManifest>),
}

impl StoredRepairManifest {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        match serde_json::from_slice::<StoredManifestVersionProbe>(bytes)?.manifest_schema_version {
            1 => match serde_json::from_slice(bytes) {
                Ok(manifest) => Ok(Self::V1(manifest)),
                Err(current_error) => serde_json::from_slice(bytes)
                    .map(Self::V1PreBaseline)
                    .map_err(|_| current_error),
            },
            2 => serde_json::from_slice(bytes).map(|manifest| Self::V2(Box::new(manifest))),
            3 => serde_json::from_slice(bytes).map(|manifest| Self::V3(Box::new(manifest))),
            4 => serde_json::from_slice(bytes).map(|manifest| Self::V4(Box::new(manifest))),
            5 => serde_json::from_slice(bytes).map(|manifest| Self::V5(Box::new(manifest))),
            6 => serde_json::from_slice(bytes).map(|manifest| Self::V6(Box::new(manifest))),
            7 => serde_json::from_slice(bytes).map(|manifest| Self::V7(Box::new(manifest))),
            version => Err(serde_json::Error::custom(format!(
                "unsupported repair manifest schema version {version}"
            ))),
        }
    }

    pub fn manifest_id(&self) -> &str {
        match self {
            Self::V1(manifest) => manifest.manifest_id(),
            Self::V1PreBaseline(manifest) => manifest.manifest_id(),
            Self::V2(manifest)
            | Self::V3(manifest)
            | Self::V4(manifest)
            | Self::V5(manifest)
            | Self::V6(manifest)
            | Self::V7(manifest) => manifest.manifest_id(),
        }
    }

    pub fn manifest_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(manifest) => StoredRepairDigestRef(manifest.manifest_digest().as_str()),
            Self::V1PreBaseline(manifest) => {
                StoredRepairDigestRef(manifest.manifest_digest().as_str())
            }
            Self::V2(manifest)
            | Self::V3(manifest)
            | Self::V4(manifest)
            | Self::V5(manifest)
            | Self::V6(manifest)
            | Self::V7(manifest) => StoredRepairDigestRef(manifest.manifest_digest().as_str()),
        }
    }

    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(manifest) => manifest.canonical_unsigned_bytes(),
            Self::V1PreBaseline(manifest) => manifest.canonical_unsigned_bytes(),
            Self::V2(manifest)
            | Self::V3(manifest)
            | Self::V4(manifest)
            | Self::V5(manifest)
            | Self::V6(manifest)
            | Self::V7(manifest) => manifest.canonical_unsigned_bytes(),
        }
    }

    pub fn rollback_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(manifest) => StoredRepairDigestRef(manifest.rollback().digest().as_str()),
            Self::V1PreBaseline(manifest) => {
                StoredRepairDigestRef(manifest.rollback().digest().as_str())
            }
            Self::V2(manifest)
            | Self::V3(manifest)
            | Self::V4(manifest)
            | Self::V5(manifest)
            | Self::V6(manifest)
            | Self::V7(manifest) => StoredRepairDigestRef(manifest.rollback().digest().as_str()),
        }
    }

    pub fn persisted_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(manifest) => serde_json::to_vec_pretty(manifest),
            Self::V1PreBaseline(manifest) => serde_json::to_vec_pretty(manifest),
            Self::V2(manifest)
            | Self::V3(manifest)
            | Self::V4(manifest)
            | Self::V5(manifest)
            | Self::V6(manifest)
            | Self::V7(manifest) => serde_json::to_vec_pretty(manifest),
        }
    }

    pub fn verify_and_try_into_current(
        self,
        verify: impl FnOnce(&[u8], StoredRepairDigestRef<'_>) -> bool,
    ) -> Result<RepairManifest, RepairContractError> {
        let canonical = self
            .canonical_unsigned_bytes()
            .map_err(|_| RepairContractError::InvalidManifest)?;
        if !verify(&canonical, self.manifest_digest()) {
            return Err(RepairContractError::InvalidDigest);
        }
        match self {
            Self::V1(manifest) => frozen_manifest_v1_into_current(manifest),
            Self::V1PreBaseline(manifest) => frozen_manifest_pre_baseline_v1_into_current(manifest),
            Self::V2(manifest)
            | Self::V3(manifest)
            | Self::V4(manifest)
            | Self::V5(manifest)
            | Self::V6(manifest)
            | Self::V7(manifest) => Ok(*manifest),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairRollbackFileKind {
    Missing,
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct RepairRollbackFileEntry {
    relative_path: String,
    kind: RepairRollbackFileKind,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    content_hex: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRollbackFileEntryWire {
    relative_path: String,
    kind: RepairRollbackFileKind,
    #[serde(default)]
    content_hex: String,
}

impl RepairRollbackFileEntry {
    pub fn missing(relative_path: String) -> Result<Self, RepairContractError> {
        Self::try_new(
            relative_path,
            RepairRollbackFileKind::Missing,
            String::new(),
        )
    }

    pub fn directory(relative_path: String) -> Result<Self, RepairContractError> {
        Self::try_new(
            relative_path,
            RepairRollbackFileKind::Directory,
            String::new(),
        )
    }

    pub fn file(relative_path: String, content: Vec<u8>) -> Result<Self, RepairContractError> {
        Self::try_new(
            relative_path,
            RepairRollbackFileKind::File,
            encode_lower_hex(&content),
        )
    }

    fn try_new(
        relative_path: String,
        kind: RepairRollbackFileKind,
        content_hex: String,
    ) -> Result<Self, RepairContractError> {
        if !valid_root_relative_path(&relative_path)
            || (kind == RepairRollbackFileKind::File
                && (!is_lower_hex(&content_hex, content_hex.len())
                    || !content_hex.len().is_multiple_of(2)))
            || (kind != RepairRollbackFileKind::File && !content_hex.is_empty())
        {
            return Err(RepairContractError::InvalidRollbackArtifact);
        }
        Ok(Self {
            relative_path,
            kind,
            content_hex,
        })
    }

    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub const fn kind(&self) -> RepairRollbackFileKind {
        self.kind
    }

    pub fn content_hex(&self) -> &str {
        &self.content_hex
    }
}

impl<'de> Deserialize<'de> for RepairRollbackFileEntry {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairRollbackFileEntryWire::deserialize(deserializer)?;
        Self::try_new(wire.relative_path, wire.kind, wire.content_hex).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairRollbackPayloadV2 {
    SingleTable {
        table: String,
        source_id: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    RenamePageTitle {
        page_id: String,
        page_columns: Vec<String>,
        before_page_row: Vec<String>,
        projection_target_path: String,
        projection_entries: Vec<RepairRollbackFileEntry>,
    },
    CompleteEntityExtraction {
        memory_id: String,
        memory_columns: Vec<String>,
        before_memory_row: Vec<String>,
        before_entity_ids: Vec<String>,
        enrichment_status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        enrichment_error: Option<String>,
        enrichment_attempts: i64,
        enrichment_updated_at: i64,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairRollbackPayloadV2Wire {
    SingleTable {
        table: String,
        source_id: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    RenamePageTitle {
        page_id: String,
        page_columns: Vec<String>,
        before_page_row: Vec<String>,
        projection_target_path: String,
        projection_entries: Vec<RepairRollbackFileEntry>,
    },
    CompleteEntityExtraction {
        memory_id: String,
        memory_columns: Vec<String>,
        before_memory_row: Vec<String>,
        before_entity_ids: Vec<String>,
        enrichment_status: String,
        #[serde(default)]
        enrichment_error: Option<String>,
        enrichment_attempts: i64,
        enrichment_updated_at: i64,
    },
}

impl RepairRollbackPayloadV2 {
    pub fn single_table(
        table: String,
        source_id: String,
        columns: Vec<String>,
        rows: Vec<Vec<String>>,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&table)
            || !valid_nonempty(&source_id)
            || !valid_columns_and_rows(&columns, &rows)
        {
            return Err(RepairContractError::InvalidRollbackArtifact);
        }
        Ok(Self::SingleTable {
            table,
            source_id,
            columns,
            rows,
        })
    }

    pub fn rename_page_title(
        page_id: String,
        page_columns: Vec<String>,
        before_page_row: Vec<String>,
        projection_target_path: String,
        projection_entries: Vec<RepairRollbackFileEntry>,
    ) -> Result<Self, RepairContractError> {
        let target_path = Path::new(&projection_target_path);
        let expected_paths = [".wenlan/state.json", projection_target_path.as_str()];
        if !valid_nonempty(&page_id)
            || page_columns.is_empty()
            || page_columns.len() != before_page_row.len()
            || !valid_unique_nonempty(&page_columns)
            || !valid_root_relative_path(&projection_target_path)
            || projection_target_path.starts_with('.')
            || target_path.components().count() != 1
            || target_path.extension().and_then(|value| value.to_str()) != Some("md")
            || projection_entries.len() != expected_paths.len()
            || !strictly_sorted_unique_by(&projection_entries, |entry| entry.relative_path())
            || projection_entries
                .iter()
                .zip(expected_paths)
                .any(|(entry, expected_path)| {
                    entry.relative_path() != expected_path
                        || entry.kind() != RepairRollbackFileKind::File
                })
        {
            return Err(RepairContractError::InvalidRollbackArtifact);
        }
        Ok(Self::RenamePageTitle {
            page_id,
            page_columns,
            before_page_row,
            projection_target_path,
            projection_entries,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn complete_entity_extraction(
        memory_id: String,
        memory_columns: Vec<String>,
        before_memory_row: Vec<String>,
        before_entity_ids: Vec<String>,
        enrichment_status: String,
        enrichment_error: Option<String>,
        enrichment_attempts: i64,
        enrichment_updated_at: i64,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&memory_id)
            || memory_columns.is_empty()
            || memory_columns.len() != before_memory_row.len()
            || !valid_unique_nonempty(&memory_columns)
            || !valid_sorted_ids(&before_entity_ids)
            || !valid_nonempty(&enrichment_status)
            || enrichment_attempts < 0
            || enrichment_updated_at <= 0
        {
            return Err(RepairContractError::InvalidRollbackArtifact);
        }
        Ok(Self::CompleteEntityExtraction {
            memory_id,
            memory_columns,
            before_memory_row,
            before_entity_ids,
            enrichment_status,
            enrichment_error,
            enrichment_attempts,
            enrichment_updated_at,
        })
    }
}

impl<'de> Deserialize<'de> for RepairRollbackPayloadV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairRollbackPayloadV2Wire::deserialize(deserializer)? {
            RepairRollbackPayloadV2Wire::SingleTable {
                table,
                source_id,
                columns,
                rows,
            } => Self::single_table(table, source_id, columns, rows),
            RepairRollbackPayloadV2Wire::RenamePageTitle {
                page_id,
                page_columns,
                before_page_row,
                projection_target_path,
                projection_entries,
            } => Self::rename_page_title(
                page_id,
                page_columns,
                before_page_row,
                projection_target_path,
                projection_entries,
            ),
            RepairRollbackPayloadV2Wire::CompleteEntityExtraction {
                memory_id,
                memory_columns,
                before_memory_row,
                before_entity_ids,
                enrichment_status,
                enrichment_error,
                enrichment_attempts,
                enrichment_updated_at,
            } => Self::complete_entity_extraction(
                memory_id,
                memory_columns,
                before_memory_row,
                before_entity_ids,
                enrichment_status,
                enrichment_error,
                enrichment_attempts,
                enrichment_updated_at,
            ),
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairRollbackV2 {
    format_version: u16,
    payload: RepairRollbackPayloadV2,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRollbackV2Wire {
    format_version: u16,
    payload: RepairRollbackPayloadV2,
}

impl RepairRollbackV2 {
    pub fn try_new(payload: RepairRollbackPayloadV2) -> Result<Self, RepairContractError> {
        Ok(Self {
            format_version: REPAIR_ROLLBACK_FORMAT_VERSION,
            payload,
        })
    }

    pub const fn format_version(&self) -> u16 {
        self.format_version
    }

    pub const fn payload(&self) -> &RepairRollbackPayloadV2 {
        &self.payload
    }
}

impl<'de> Deserialize<'de> for RepairRollbackV2 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairRollbackV2Wire::deserialize(deserializer)?;
        if wire.format_version != REPAIR_ROLLBACK_FORMAT_VERSION {
            return Err(D::Error::custom(
                RepairContractError::InvalidRollbackArtifact,
            ));
        }
        Self::try_new(wire.payload).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairRollbackPayloadV3 {
    EntityRelation { snapshot: RepairRelationSnapshot },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairRollbackPayloadV3Wire {
    EntityRelation { snapshot: RepairRelationSnapshot },
}

impl RepairRollbackPayloadV3 {
    pub fn entity_relation(snapshot: RepairRelationSnapshot) -> Result<Self, RepairContractError> {
        snapshot
            .validate()
            .map_err(|_| RepairContractError::InvalidRollbackArtifact)?;
        Ok(Self::EntityRelation { snapshot })
    }

    pub fn snapshot(&self) -> &RepairRelationSnapshot {
        match self {
            Self::EntityRelation { snapshot } => snapshot,
        }
    }
}

impl<'de> Deserialize<'de> for RepairRollbackPayloadV3 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairRollbackPayloadV3Wire::deserialize(deserializer)? {
            RepairRollbackPayloadV3Wire::EntityRelation { snapshot } => {
                Self::entity_relation(snapshot)
            }
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairRollbackV3 {
    format_version: u16,
    payload: RepairRollbackPayloadV3,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRollbackV3Wire {
    format_version: u16,
    payload: RepairRollbackPayloadV3,
}

impl RepairRollbackV3 {
    pub fn try_new(payload: RepairRollbackPayloadV3) -> Result<Self, RepairContractError> {
        payload
            .snapshot()
            .validate()
            .map_err(|_| RepairContractError::InvalidRollbackArtifact)?;
        Ok(Self {
            format_version: REPAIR_RELATION_ROLLBACK_FORMAT_VERSION,
            payload,
        })
    }

    pub const fn format_version(&self) -> u16 {
        self.format_version
    }

    pub const fn payload(&self) -> &RepairRollbackPayloadV3 {
        &self.payload
    }
}

impl<'de> Deserialize<'de> for RepairRollbackV3 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairRollbackV3Wire::deserialize(deserializer)?;
        if wire.format_version != REPAIR_RELATION_ROLLBACK_FORMAT_VERSION {
            return Err(D::Error::custom(
                RepairContractError::InvalidRollbackArtifact,
            ));
        }
        Self::try_new(wire.payload).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredRepairRollbackArtifact {
    V1(FrozenRepairRollbackArtifactV1),
    V2(RepairRollbackV2),
    V3(RepairRollbackV3),
}

impl StoredRepairRollbackArtifact {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        match serde_json::from_slice::<StoredRollbackVersionProbe>(bytes)?.format_version {
            1 => serde_json::from_slice(bytes).map(Self::V1),
            2 => serde_json::from_slice(bytes).map(Self::V2),
            3 => serde_json::from_slice(bytes).map(Self::V3),
            version => Err(serde_json::Error::custom(format!(
                "unsupported repair rollback format version {version}"
            ))),
        }
    }

    pub fn persisted_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(rollback) => serde_json::to_vec_pretty(rollback),
            Self::V2(rollback) => serde_json::to_vec_pretty(rollback),
            Self::V3(rollback) => serde_json::to_vec_pretty(rollback),
        }
    }

    pub fn as_v1(&self) -> &FrozenRepairRollbackArtifactV1 {
        match self {
            Self::V1(rollback) => rollback,
            Self::V2(_) | Self::V3(_) => panic!("repair rollback is not frozen v1"),
        }
    }

    pub const fn as_v2(&self) -> Option<&RepairRollbackV2> {
        match self {
            Self::V1(_) | Self::V3(_) => None,
            Self::V2(rollback) => Some(rollback),
        }
    }

    pub const fn as_v3(&self) -> Option<&RepairRollbackV3> {
        match self {
            Self::V3(rollback) => Some(rollback),
            Self::V1(_) | Self::V2(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredRepairApplyReceipt {
    V1(FrozenRepairApplyReceiptV1),
    V2(RepairApplyReceipt),
    V3(RepairApplyReceipt),
    V4(RepairApplyReceipt),
    V5(RepairApplyReceipt),
    V6(RepairApplyReceipt),
}

impl StoredRepairApplyReceipt {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        match serde_json::from_slice::<StoredReceiptVersionProbe>(bytes)?.receipt_schema_version {
            1 => serde_json::from_slice(bytes).map(Self::V1),
            2 => serde_json::from_slice(bytes).map(Self::V2),
            3 => serde_json::from_slice(bytes).map(Self::V3),
            4 => serde_json::from_slice(bytes).map(Self::V4),
            5 => serde_json::from_slice(bytes).map(Self::V5),
            6 => serde_json::from_slice(bytes).map(Self::V6),
            version => Err(serde_json::Error::custom(format!(
                "unsupported repair receipt schema version {version}"
            ))),
        }
    }

    pub fn receipt_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(receipt) => StoredRepairDigestRef(receipt.receipt_digest().as_str()),
            Self::V2(receipt)
            | Self::V3(receipt)
            | Self::V4(receipt)
            | Self::V5(receipt)
            | Self::V6(receipt) => StoredRepairDigestRef(receipt.receipt_digest().as_str()),
        }
    }

    pub fn manifest_id(&self) -> &str {
        match self {
            Self::V1(receipt) => receipt.manifest_id(),
            Self::V2(receipt)
            | Self::V3(receipt)
            | Self::V4(receipt)
            | Self::V5(receipt)
            | Self::V6(receipt) => receipt.manifest_id(),
        }
    }

    pub fn manifest_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(receipt) => StoredRepairDigestRef(receipt.manifest_digest().as_str()),
            Self::V2(receipt)
            | Self::V3(receipt)
            | Self::V4(receipt)
            | Self::V5(receipt)
            | Self::V6(receipt) => StoredRepairDigestRef(receipt.manifest_digest().as_str()),
        }
    }

    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(receipt) => receipt.canonical_unsigned_bytes(),
            Self::V2(receipt)
            | Self::V3(receipt)
            | Self::V4(receipt)
            | Self::V5(receipt)
            | Self::V6(receipt) => receipt.canonical_unsigned_bytes(),
        }
    }

    pub fn persisted_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(receipt) => serde_json::to_vec_pretty(receipt),
            Self::V2(receipt)
            | Self::V3(receipt)
            | Self::V4(receipt)
            | Self::V5(receipt)
            | Self::V6(receipt) => serde_json::to_vec_pretty(receipt),
        }
    }

    pub fn verify_and_try_into_current(
        self,
        verify: impl FnOnce(&[u8], StoredRepairDigestRef<'_>) -> bool,
    ) -> Result<RepairApplyReceipt, RepairContractError> {
        let canonical = self
            .canonical_unsigned_bytes()
            .map_err(|_| RepairContractError::InvalidReceipt)?;
        if !verify(&canonical, self.receipt_digest()) {
            return Err(RepairContractError::InvalidDigest);
        }
        match self {
            Self::V1(receipt) => frozen_apply_receipt_v1_into_current(receipt),
            Self::V2(receipt)
            | Self::V3(receipt)
            | Self::V4(receipt)
            | Self::V5(receipt)
            | Self::V6(receipt) => Ok(receipt),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoredRepairVerificationReceipt {
    V1(FrozenRepairVerificationReceiptV1),
    V2(RepairVerificationReceipt),
    V3(RepairVerificationReceipt),
    V4(RepairVerificationReceipt),
    V5(RepairVerificationReceipt),
}

impl StoredRepairVerificationReceipt {
    pub fn from_slice(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        match serde_json::from_slice::<StoredReceiptVersionProbe>(bytes)?.receipt_schema_version {
            1 => serde_json::from_slice(bytes).map(Self::V1),
            2 => serde_json::from_slice(bytes).map(Self::V2),
            3 => serde_json::from_slice(bytes).map(Self::V3),
            4 => serde_json::from_slice(bytes).map(Self::V4),
            5 => serde_json::from_slice(bytes).map(Self::V5),
            version => Err(serde_json::Error::custom(format!(
                "unsupported repair receipt schema version {version}"
            ))),
        }
    }

    pub fn receipt_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(receipt) => StoredRepairDigestRef(receipt.receipt_digest().as_str()),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                StoredRepairDigestRef(receipt.receipt_digest().as_str())
            }
        }
    }

    pub fn manifest_id(&self) -> &str {
        match self {
            Self::V1(receipt) => receipt.manifest_id(),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                receipt.manifest_id()
            }
        }
    }

    pub fn manifest_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(receipt) => StoredRepairDigestRef(receipt.manifest_digest().as_str()),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                StoredRepairDigestRef(receipt.manifest_digest().as_str())
            }
        }
    }

    pub fn apply_receipt_digest(&self) -> StoredRepairDigestRef<'_> {
        match self {
            Self::V1(receipt) => StoredRepairDigestRef(receipt.apply_receipt_digest().as_str()),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                StoredRepairDigestRef(receipt.apply_receipt_digest().as_str())
            }
        }
    }

    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(receipt) => receipt.canonical_unsigned_bytes(),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                receipt.canonical_unsigned_bytes()
            }
        }
    }

    pub fn persisted_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        match self {
            Self::V1(receipt) => serde_json::to_vec_pretty(receipt),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                serde_json::to_vec_pretty(receipt)
            }
        }
    }

    pub fn verify_and_try_into_current(
        self,
        verify: impl FnOnce(&[u8], StoredRepairDigestRef<'_>) -> bool,
    ) -> Result<RepairVerificationReceipt, RepairContractError> {
        let canonical = self
            .canonical_unsigned_bytes()
            .map_err(|_| RepairContractError::InvalidReceipt)?;
        if !verify(&canonical, self.receipt_digest()) {
            return Err(RepairContractError::InvalidDigest);
        }
        match self {
            Self::V1(receipt) => frozen_verification_receipt_v1_into_current(receipt),
            Self::V2(receipt) | Self::V3(receipt) | Self::V4(receipt) | Self::V5(receipt) => {
                Ok(receipt)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepairContractError {
    InvalidDigest,
    InvalidManifestId,
    InvalidSource,
    InvalidTarget,
    InvalidExpectedState,
    UnsupportedWriter,
    InvalidMutation,
    InvalidAllowedEffects,
    InvalidRollbackArtifact,
    InvalidPostAssertions,
    UnsupportedManifestSchema,
    InvalidManifest,
    InvalidPrepareRequest,
    InvalidApplyRequest,
    InvalidVerifyRequest,
    InvalidReceipt,
}

impl RepairContractError {
    const fn code(self) -> &'static str {
        match self {
            Self::InvalidDigest => "invalid_repair_digest",
            Self::InvalidManifestId => "invalid_repair_manifest_id",
            Self::InvalidSource => "invalid_repair_source",
            Self::InvalidTarget => "invalid_repair_target",
            Self::InvalidExpectedState => "invalid_repair_expected_state",
            Self::UnsupportedWriter => "unsupported_repair_writer",
            Self::InvalidMutation => "invalid_repair_mutation",
            Self::InvalidAllowedEffects => "invalid_repair_allowed_effects",
            Self::InvalidRollbackArtifact => "invalid_repair_rollback_artifact",
            Self::InvalidPostAssertions => "invalid_repair_post_assertions",
            Self::UnsupportedManifestSchema => "unsupported_repair_manifest_schema",
            Self::InvalidManifest => "invalid_repair_manifest",
            Self::InvalidPrepareRequest => "invalid_prepare_repair_request",
            Self::InvalidApplyRequest => "invalid_apply_repair_request",
            Self::InvalidVerifyRequest => "invalid_verify_repair_request",
            Self::InvalidReceipt => "invalid_repair_receipt",
        }
    }
}

impl fmt::Display for RepairContractError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

impl std::error::Error for RepairContractError {}

fn frozen_repair_digest_v1(
    digest: frozen_v1::FrozenRepairDigestV1,
) -> Result<RepairDigest, RepairContractError> {
    RepairDigest::parse(&digest.0)
}

fn frozen_lint_digest_v1(
    digest: frozen_v1::FrozenLintDigestV1,
) -> Result<LintDigest, RepairContractError> {
    LintDigest::from_hex(&digest.0).map_err(|_| RepairContractError::InvalidSource)
}

fn frozen_lint_opaque_id_v1(
    id: frozen_v1::FrozenLintOpaqueIdV1,
) -> Result<LintOpaqueId, RepairContractError> {
    let position =
        id.0.checked_sub(1)
            .and_then(|value| usize::try_from(value).ok())
            .and_then(LintOpaqueId::from_sorted_position)
            .ok_or(RepairContractError::InvalidSource)?;
    Ok(position)
}

fn frozen_lint_scope_v1(
    scope: frozen_v1::FrozenLintScopeV1,
) -> Result<LintScope, RepairContractError> {
    match (scope.kind, scope.opaque_scope_ref) {
        (frozen_v1::FrozenLintScopeKindV1::Global, None) => Ok(LintScope::global()),
        (frozen_v1::FrozenLintScopeKindV1::Registered, Some(reference)) => {
            Ok(LintScope::registered(frozen_lint_opaque_id_v1(reference)?))
        }
        (frozen_v1::FrozenLintScopeKindV1::Uncategorized, None) => Ok(LintScope::uncategorized()),
        _ => Err(RepairContractError::InvalidSource),
    }
}

fn frozen_lint_snapshots_v1(
    snapshots: frozen_v1::FrozenLintSnapshotReceiptsV1,
) -> Result<LintSnapshotReceipts, RepairContractError> {
    let db = snapshots.db;
    let db_mode = match db.mode {
        frozen_v1::FrozenLintDbSnapshotModeV1::TransactionalReadOnly => {
            LintDbSnapshotMode::TransactionalReadOnly
        }
    };
    let pages = snapshots.pages;
    let page_mode = match pages.mode {
        frozen_v1::FrozenLintPageSnapshotModeV1::BestEffort => LintPageSnapshotMode::BestEffort,
    };
    Ok(LintSnapshotReceipts::new(
        LintDbSnapshotReceipt::new(
            db_mode,
            frozen_lint_digest_v1(db.analysis_digest)?,
            db.post_run_digest.map(frozen_lint_digest_v1).transpose()?,
        ),
        LintPageSnapshotReceipt::new(
            page_mode,
            frozen_lint_digest_v1(pages.before_scan_digest)?,
            pages
                .after_scan_digest
                .map(frozen_lint_digest_v1)
                .transpose()?,
        ),
    ))
}

fn frozen_lint_producer_receipt_v1(
    receipt: frozen_v1::FrozenLintProducerReceiptV1,
) -> Result<LintProducerReceipt, RepairContractError> {
    let commit = receipt
        .runtime_commit
        .map(|commit| {
            LintCommitReceipt::new(&commit.0).map_err(|_| RepairContractError::InvalidSource)
        })
        .transpose()?;
    Ok(LintProducerReceipt::new(commit))
}

fn frozen_semantic_action_v1(action: frozen_v1::FrozenLintSemanticActionV1) -> LintSemanticAction {
    match action {
        frozen_v1::FrozenLintSemanticActionV1::ReclassifyMemory => {
            LintSemanticAction::ReclassifyMemory
        }
        frozen_v1::FrozenLintSemanticActionV1::ReviewContradiction => {
            LintSemanticAction::ReviewContradiction
        }
        frozen_v1::FrozenLintSemanticActionV1::ReviewStaleness => {
            LintSemanticAction::ReviewStaleness
        }
        frozen_v1::FrozenLintSemanticActionV1::SupersedeMemory => {
            LintSemanticAction::SupersedeMemory
        }
        frozen_v1::FrozenLintSemanticActionV1::AddMemoryEntityLink => {
            LintSemanticAction::AddMemoryEntityLink
        }
        frozen_v1::FrozenLintSemanticActionV1::RemoveMemoryEntityLink => {
            LintSemanticAction::RemoveMemoryEntityLink
        }
        frozen_v1::FrozenLintSemanticActionV1::AddEntityRelation => {
            LintSemanticAction::AddEntityRelation
        }
        frozen_v1::FrozenLintSemanticActionV1::RemoveEntityRelation => {
            LintSemanticAction::RemoveEntityRelation
        }
        frozen_v1::FrozenLintSemanticActionV1::ReviewPageClaim => {
            LintSemanticAction::ReviewPageClaim
        }
        frozen_v1::FrozenLintSemanticActionV1::AddPageEvidence => {
            LintSemanticAction::AddPageEvidence
        }
        frozen_v1::FrozenLintSemanticActionV1::RemovePageEvidence => {
            LintSemanticAction::RemovePageEvidence
        }
        frozen_v1::FrozenLintSemanticActionV1::ReviewRetrieval => {
            LintSemanticAction::ReviewRetrieval
        }
    }
}

fn frozen_semantic_reason_v1(
    reason: frozen_v1::FrozenLintSemanticReasonCodeV1,
) -> LintSemanticReasonCode {
    match reason {
        frozen_v1::FrozenLintSemanticReasonCodeV1::ClassificationMismatch => {
            LintSemanticReasonCode::ClassificationMismatch
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::PotentialContradiction => {
            LintSemanticReasonCode::PotentialContradiction
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::PotentialStaleness => {
            LintSemanticReasonCode::PotentialStaleness
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::MentionWithoutLink => {
            LintSemanticReasonCode::MentionWithoutLink
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::ExistingLinkMismatch => {
            LintSemanticReasonCode::ExistingLinkMismatch
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::SharedContextWithoutRelation => {
            LintSemanticReasonCode::SharedContextWithoutRelation
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::ExistingRelationMismatch => {
            LintSemanticReasonCode::ExistingRelationMismatch
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::PotentialUnfaithfulClaim => {
            LintSemanticReasonCode::PotentialUnfaithfulClaim
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::PotentialInadequateProvenance => {
            LintSemanticReasonCode::PotentialInadequateProvenance
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::ClaimOverlapWithoutEvidence => {
            LintSemanticReasonCode::ClaimOverlapWithoutEvidence
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::ExistingEvidenceMismatch => {
            LintSemanticReasonCode::ExistingEvidenceMismatch
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::PotentialRetrievalMiss => {
            LintSemanticReasonCode::PotentialRetrievalMiss
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::DanglingOwner => {
            LintSemanticReasonCode::DanglingOwner
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::TemporalEvolution => {
            LintSemanticReasonCode::TemporalEvolution
        }
        frozen_v1::FrozenLintSemanticReasonCodeV1::RelatedButNotEvidence => {
            LintSemanticReasonCode::RelatedButNotEvidence
        }
    }
}

fn frozen_semantic_provider_v1(
    provider: frozen_v1::FrozenLintSemanticProviderRouteV1,
) -> LintSemanticProviderRoute {
    match provider {
        frozen_v1::FrozenLintSemanticProviderRouteV1::OnDevice => {
            LintSemanticProviderRoute::OnDevice
        }
        frozen_v1::FrozenLintSemanticProviderRouteV1::ConfiguredExternal => {
            LintSemanticProviderRoute::ConfiguredExternal
        }
        frozen_v1::FrozenLintSemanticProviderRouteV1::CallingAgent => {
            LintSemanticProviderRoute::CallingAgent
        }
    }
}

fn frozen_semantic_finding_v1(
    finding: frozen_v1::FrozenLintSemanticFindingV1,
) -> Result<LintSemanticFinding, RepairContractError> {
    LintSemanticFinding::try_new_with_disagreement(
        frozen_lint_opaque_id_v1(finding.candidate_id)?,
        frozen_semantic_action_v1(finding.proposed_action),
        frozen_semantic_reason_v1(finding.reason_code),
        finding.confidence_basis_points,
        frozen_semantic_provider_v1(finding.provider_route),
        finding
            .evidence_ids
            .into_iter()
            .map(frozen_lint_digest_v1)
            .collect::<Result<Vec<_>, _>>()?,
        finding
            .counterevidence_ids
            .into_iter()
            .map(frozen_lint_digest_v1)
            .collect::<Result<Vec<_>, _>>()?,
        finding.unresolved_disagreement,
    )
    .map_err(|_| RepairContractError::InvalidSource)
}

fn frozen_lint_outcome_v1(outcome: frozen_v1::FrozenLintOutcomeV1) -> LintOutcome {
    match outcome {
        frozen_v1::FrozenLintOutcomeV1::Pass => LintOutcome::Pass,
        frozen_v1::FrozenLintOutcomeV1::Finding => LintOutcome::Finding,
        frozen_v1::FrozenLintOutcomeV1::NotRunPrerequisite => LintOutcome::NotRunPrerequisite,
        frozen_v1::FrozenLintOutcomeV1::InconsistentSnapshot => LintOutcome::InconsistentSnapshot,
        frozen_v1::FrozenLintOutcomeV1::FailedToRun => LintOutcome::FailedToRun,
    }
}

fn frozen_lint_gate_effect_v1(effect: frozen_v1::FrozenLintGateEffectV1) -> LintGateEffect {
    match effect {
        frozen_v1::FrozenLintGateEffectV1::Actionable => LintGateEffect::Actionable,
        frozen_v1::FrozenLintGateEffectV1::Advisory => LintGateEffect::Advisory,
    }
}

fn frozen_lint_reason_v1(reason: frozen_v1::FrozenLintReasonCodeV1) -> LintReasonCode {
    match reason {
        frozen_v1::FrozenLintReasonCodeV1::MissingArtifact => LintReasonCode::MissingArtifact,
        frozen_v1::FrozenLintReasonCodeV1::InvalidCatalogState => {
            LintReasonCode::InvalidCatalogState
        }
        frozen_v1::FrozenLintReasonCodeV1::ExpectedEmptySubstrate => {
            LintReasonCode::ExpectedEmptySubstrate
        }
        frozen_v1::FrozenLintReasonCodeV1::InvalidSourceConfiguration => {
            LintReasonCode::InvalidSourceConfiguration
        }
        frozen_v1::FrozenLintReasonCodeV1::TerminalOperationFailure => {
            LintReasonCode::TerminalOperationFailure
        }
        frozen_v1::FrozenLintReasonCodeV1::ExpiredRetry => LintReasonCode::ExpiredRetry,
        frozen_v1::FrozenLintReasonCodeV1::InvalidOperationState => {
            LintReasonCode::InvalidOperationState
        }
        frozen_v1::FrozenLintReasonCodeV1::DurableNoProgress => LintReasonCode::DurableNoProgress,
        frozen_v1::FrozenLintReasonCodeV1::SemanticProviderUnavailable => {
            LintReasonCode::SemanticProviderUnavailable
        }
        frozen_v1::FrozenLintReasonCodeV1::InsufficientSemanticEvidence => {
            LintReasonCode::InsufficientSemanticEvidence
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticExecutionFailure => {
            LintReasonCode::SemanticExecutionFailure
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticAgentAdjudicationRequired => {
            LintReasonCode::SemanticAgentAdjudicationRequired
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticAgentWorkStale => {
            LintReasonCode::SemanticAgentWorkStale
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticAgentSubmissionInvalid => {
            LintReasonCode::SemanticAgentSubmissionInvalid
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticCandidateGenerationFailure => {
            LintReasonCode::SemanticCandidateGenerationFailure
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticPopulationIncomplete => {
            LintReasonCode::SemanticPopulationIncomplete
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticDisagreementUnresolved => {
            LintReasonCode::SemanticDisagreementUnresolved
        }
        frozen_v1::FrozenLintReasonCodeV1::SemanticSecondJudgeRequired => {
            LintReasonCode::SemanticSecondJudgeRequired
        }
    }
}

fn frozen_lint_safe_path_v1(
    path: frozen_v1::FrozenLintSafeRootRelativePathV1,
) -> LintSafeRootRelativePath {
    match path {
        frozen_v1::FrozenLintSafeRootRelativePathV1::PagesRoot => {
            LintSafeRootRelativePath::PagesRoot
        }
        frozen_v1::FrozenLintSafeRootRelativePathV1::PagesState => {
            LintSafeRootRelativePath::PagesState
        }
        frozen_v1::FrozenLintSafeRootRelativePathV1::PagesManifest => {
            LintSafeRootRelativePath::PagesManifest
        }
        frozen_v1::FrozenLintSafeRootRelativePathV1::PagesStubs => {
            LintSafeRootRelativePath::PagesStubs
        }
    }
}

fn frozen_lint_evidence_v1(
    evidence: frozen_v1::FrozenLintEvidenceRefV1,
) -> Result<LintEvidenceRef, RepairContractError> {
    match evidence {
        frozen_v1::FrozenLintEvidenceRefV1::OpaqueId { opaque_id } => {
            Ok(LintEvidenceRef::OpaqueId {
                opaque_id: frozen_lint_opaque_id_v1(opaque_id)?,
            })
        }
        frozen_v1::FrozenLintEvidenceRefV1::ReasonCode { reason_code } => {
            Ok(LintEvidenceRef::ReasonCode {
                reason_code: frozen_lint_reason_v1(reason_code),
            })
        }
        frozen_v1::FrozenLintEvidenceRefV1::SafeRootRelativePath {
            safe_root_relative_path,
        } => Ok(LintEvidenceRef::SafeRootRelativePath {
            safe_root_relative_path: frozen_lint_safe_path_v1(safe_root_relative_path),
        }),
        frozen_v1::FrozenLintEvidenceRefV1::SemanticFinding { finding } => {
            Ok(LintEvidenceRef::SemanticFinding {
                finding: frozen_semantic_finding_v1(finding)?,
            })
        }
    }
}

fn frozen_repair_lint_scope_v1(
    scope: frozen_v1::FrozenRepairLintScopeV1,
) -> Result<RepairLintScope, RepairContractError> {
    match scope {
        frozen_v1::FrozenRepairLintScopeV1::Global {} => Ok(RepairLintScope::global()),
        frozen_v1::FrozenRepairLintScopeV1::Registered { space } => {
            RepairLintScope::registered(space)
        }
        frozen_v1::FrozenRepairLintScopeV1::Uncategorized {} => {
            Ok(RepairLintScope::uncategorized())
        }
    }
}

fn frozen_repair_scope_v1(
    scope: frozen_v1::FrozenRepairScopeV1,
) -> Result<RepairScope, RepairContractError> {
    match scope {
        frozen_v1::FrozenRepairScopeV1::Registered { space } => RepairScope::registered(space),
        frozen_v1::FrozenRepairScopeV1::Uncategorized {} => Ok(RepairScope::uncategorized()),
    }
}

fn frozen_repair_target_v1(
    target: frozen_v1::FrozenRepairTargetV1,
) -> Result<RepairTarget, RepairContractError> {
    match target {
        frozen_v1::FrozenRepairTargetV1::Memory { source_id, scope } => {
            RepairTarget::memory(source_id, frozen_repair_scope_v1(scope)?)
        }
    }
}

fn frozen_repair_source_v1(
    source: frozen_v1::FrozenRepairSourceV1,
) -> Result<RepairSource, RepairContractError> {
    if source.report_schema_version != frozen_v1::FROZEN_LINT_REPORT_SCHEMA_VERSION_V1
        || source.check_catalog_version != frozen_v1::FROZEN_LINT_CHECK_CATALOG_VERSION_V1
        || source.check_id != REPAIR_CLASSIFICATION_CHECK_ID
    {
        return Err(RepairContractError::InvalidSource);
    }
    RepairSource::try_new(
        frozen_repair_lint_scope_v1(source.lint_scope)?,
        frozen_lint_scope_v1(source.report_scope)?,
        frozen_semantic_finding_v1(source.finding)?,
        frozen_lint_snapshots_v1(source.general_snapshots)?,
        frozen_lint_snapshots_v1(source.deep_snapshots)?,
        frozen_lint_producer_receipt_v1(source.general_producer_receipt)?,
        frozen_lint_producer_receipt_v1(source.deep_producer_receipt)?,
        frozen_lint_digest_v1(source.agent_work_digest)?,
    )
}

fn frozen_expected_state_v1(
    expected: frozen_v1::FrozenRepairExpectedStateV1,
) -> Result<RepairExpectedState, RepairContractError> {
    RepairExpectedState::try_new(
        expected.version,
        frozen_repair_digest_v1(expected.canonical_receipt)?,
    )
}

fn frozen_writer_v1(writer: frozen_v1::FrozenRepairWriterV1) -> RepairWriter {
    match writer {
        frozen_v1::FrozenRepairWriterV1::ReclassifyMemory => RepairWriter::ReclassifyMemory,
    }
}

fn frozen_memory_type_v1(memory_type: frozen_v1::FrozenMemoryTypeV1) -> MemoryType {
    match memory_type {
        frozen_v1::FrozenMemoryTypeV1::Identity => MemoryType::Identity,
        frozen_v1::FrozenMemoryTypeV1::Preference => MemoryType::Preference,
        frozen_v1::FrozenMemoryTypeV1::Decision => MemoryType::Decision,
        frozen_v1::FrozenMemoryTypeV1::Lesson => MemoryType::Lesson,
        frozen_v1::FrozenMemoryTypeV1::Gotcha => MemoryType::Gotcha,
        frozen_v1::FrozenMemoryTypeV1::Fact => MemoryType::Fact,
    }
}

fn frozen_mutation_v1(
    mutation: frozen_v1::FrozenRepairMutationV1,
) -> Result<RepairMutation, RepairContractError> {
    match mutation {
        frozen_v1::FrozenRepairMutationV1::ReclassifyMemory {
            before_memory_type,
            after_memory_type,
        } => RepairMutation::from_memory_types(
            before_memory_type.map(frozen_memory_type_v1),
            frozen_memory_type_v1(after_memory_type),
        ),
    }
}

fn frozen_allowed_effects_v1(
    effects: frozen_v1::FrozenRepairAllowedEffectsV1,
) -> Result<RepairAllowedEffects, RepairContractError> {
    if effects.fields != [frozen_v1::FrozenRepairMemoryFieldV1::MemoryType] {
        return Err(RepairContractError::InvalidAllowedEffects);
    }
    Ok(RepairAllowedEffects::memory_type(frozen_repair_target_v1(
        effects.owner,
    )?))
}

fn frozen_rollback_reference_v1(
    rollback: frozen_v1::FrozenRepairRollbackReferenceV1,
) -> Result<RepairRollbackArtifact, RepairContractError> {
    if rollback.format_version != 1 {
        return Err(RepairContractError::InvalidRollbackArtifact);
    }
    RepairRollbackArtifact::try_new_for_format(
        1,
        rollback.relative_path,
        frozen_repair_digest_v1(rollback.digest)?,
    )
}

fn frozen_check_baseline_v1(
    baseline: frozen_v1::FrozenRepairCheckBaselineV1,
) -> Result<RepairCheckBaseline, RepairContractError> {
    RepairCheckBaseline::try_new(
        baseline.check_id,
        frozen_lint_outcome_v1(baseline.outcome),
        frozen_lint_gate_effect_v1(baseline.gate_effect),
        baseline
            .evidence
            .into_iter()
            .map(frozen_lint_evidence_v1)
            .collect::<Result<Vec<_>, _>>()?,
    )
}

fn frozen_post_assertions_v1(
    assertions: frozen_v1::FrozenRepairPostAssertionsV1,
) -> Result<RepairPostAssertions, RepairContractError> {
    if assertions.target_check_id != REPAIR_CLASSIFICATION_CHECK_ID
        || !assertions.require_complete_general
        || !assertions.require_complete_deep
        || !assertions.reject_new_actionable
        || !assertions.reject_new_incomplete
    {
        return Err(RepairContractError::InvalidPostAssertions);
    }
    RepairPostAssertions::try_new_legacy_v1(
        frozen_lint_digest_v1(assertions.target_evidence_id)?,
        assertions
            .general_baseline
            .into_iter()
            .map(frozen_check_baseline_v1)
            .collect::<Result<Vec<_>, _>>()?,
        assertions
            .deep_baseline
            .into_iter()
            .map(frozen_check_baseline_v1)
            .collect::<Result<Vec<_>, _>>()?,
        assertions.allowed_non_target_check_deltas,
    )
}

fn frozen_post_assertions_pre_baseline_v1(
    assertions: frozen_v1::FrozenRepairPostAssertionsPreBaselineV1,
) -> Result<RepairPostAssertions, RepairContractError> {
    if assertions.target_check_id != REPAIR_CLASSIFICATION_CHECK_ID
        || !assertions.require_complete_general
        || !assertions.require_complete_deep
        || !assertions.reject_new_actionable
        || !assertions.reject_new_incomplete
        || assertions
            .allowed_non_target_check_deltas
            .iter()
            .any(|value| !valid_nonempty(value))
        || !assertions
            .allowed_non_target_check_deltas
            .windows(2)
            .all(|pair| pair[0] < pair[1])
    {
        return Err(RepairContractError::InvalidPostAssertions);
    }
    Ok(RepairPostAssertions {
        target_check_id: REPAIR_CLASSIFICATION_CHECK_ID.to_string(),
        target_evidence_id: frozen_lint_digest_v1(assertions.target_evidence_id)?,
        general_baseline: Vec::new(),
        deep_baseline: Vec::new(),
        target_record_set: None,
        verification_policy: RepairVerificationPolicy::LegacyWholeReports,
        require_complete_general: true,
        reject_new_actionable: true,
        reject_new_incomplete: true,
        allowed_non_target_check_deltas: assertions.allowed_non_target_check_deltas,
    })
}

fn frozen_manifest_v1_into_current(
    manifest: FrozenRepairManifestV1,
) -> Result<RepairManifest, RepairContractError> {
    let frozen_v1::FrozenRepairManifestV1 {
        draft,
        manifest_digest,
    } = manifest;
    if draft.manifest_schema_version != 1 {
        return Err(RepairContractError::UnsupportedManifestSchema);
    }
    let draft = RepairManifestDraft::try_new(
        draft.manifest_id,
        draft.prepared_at,
        frozen_repair_source_v1(draft.source)?,
        frozen_repair_target_v1(draft.target)?,
        frozen_expected_state_v1(draft.expected_state)?,
        frozen_writer_v1(draft.writer),
        frozen_mutation_v1(draft.mutation)?,
        frozen_allowed_effects_v1(draft.allowed_effects)?,
        frozen_rollback_reference_v1(draft.rollback)?,
        frozen_post_assertions_v1(draft.post_assertions)?,
    )?;
    RepairManifest::try_new(draft, frozen_repair_digest_v1(manifest_digest)?)
}

fn frozen_manifest_pre_baseline_v1_into_current(
    manifest: FrozenRepairManifestPreBaselineV1,
) -> Result<RepairManifest, RepairContractError> {
    let frozen_v1::FrozenRepairManifestPreBaselineV1 {
        draft,
        manifest_digest,
    } = manifest;
    if draft.manifest_schema_version != 1 {
        return Err(RepairContractError::UnsupportedManifestSchema);
    }
    let draft = RepairManifestDraft::try_new(
        draft.manifest_id,
        draft.prepared_at,
        frozen_repair_source_v1(draft.source)?,
        frozen_repair_target_v1(draft.target)?,
        frozen_expected_state_v1(draft.expected_state)?,
        frozen_writer_v1(draft.writer),
        frozen_mutation_v1(draft.mutation)?,
        frozen_allowed_effects_v1(draft.allowed_effects)?,
        frozen_rollback_reference_v1(draft.rollback)?,
        frozen_post_assertions_pre_baseline_v1(draft.post_assertions)?,
    )?;
    RepairManifest::try_new(draft, frozen_repair_digest_v1(manifest_digest)?)
}

fn frozen_apply_receipt_v1_into_current(
    receipt: FrozenRepairApplyReceiptV1,
) -> Result<RepairApplyReceipt, RepairContractError> {
    let frozen_v1::FrozenRepairApplyReceiptV1 {
        draft,
        receipt_digest,
    } = receipt;
    if draft.receipt_schema_version != 1 {
        return Err(RepairContractError::InvalidReceipt);
    }
    let draft = RepairApplyReceiptDraft::try_new_legacy_v1(
        draft.manifest_id,
        frozen_repair_digest_v1(draft.manifest_digest)?,
        draft.applied_at,
        frozen_repair_digest_v1(draft.before_target_receipt)?,
        frozen_repair_digest_v1(draft.after_target_receipt)?,
        frozen_repair_digest_v1(draft.non_target_before)?,
        frozen_repair_digest_v1(draft.non_target_after)?,
        frozen_allowed_effects_v1(draft.actual_effects)?,
        frozen_writer_v1(draft.writer),
    )?;
    Ok(RepairApplyReceipt::from_draft(
        draft,
        frozen_repair_digest_v1(receipt_digest)?,
    ))
}

fn frozen_verification_receipt_v1_into_current(
    receipt: FrozenRepairVerificationReceiptV1,
) -> Result<RepairVerificationReceipt, RepairContractError> {
    let frozen_v1::FrozenRepairVerificationReceiptV1 {
        draft,
        receipt_digest,
    } = receipt;
    if draft.receipt_schema_version != 1 {
        return Err(RepairContractError::InvalidReceipt);
    }
    let draft = RepairVerificationReceiptDraft::try_new_legacy_v1(
        draft.manifest_id,
        frozen_repair_digest_v1(draft.manifest_digest)?,
        frozen_repair_digest_v1(draft.apply_receipt_digest)?,
        draft.verified_at,
        frozen_lint_snapshots_v1(draft.general_snapshots)?,
        frozen_lint_snapshots_v1(draft.deep_snapshots)?,
    )?;
    Ok(RepairVerificationReceipt::from_draft(
        draft,
        frozen_repair_digest_v1(receipt_digest)?,
    ))
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value.bytes().all(|byte| {
            byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit())
        })
}

fn valid_manifest_id(value: &str) -> bool {
    let Some(uuid) = value.strip_prefix("repair_") else {
        return false;
    };
    uuid.len() == 36
        && uuid.bytes().enumerate().all(|(index, byte)| match index {
            8 | 13 | 18 | 23 => byte == b'-',
            _ => byte.is_ascii_digit() || (byte.is_ascii_lowercase() && byte.is_ascii_hexdigit()),
        })
}

fn valid_nonempty(value: &str) -> bool {
    !value.is_empty() && value.trim() == value
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}

fn strictly_sorted_unique(values: &[String]) -> bool {
    !values.is_empty()
        && values.iter().all(|value| valid_nonempty(value))
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn strictly_sorted_unique_by<T>(values: &[T], key: impl Fn(&T) -> &str) -> bool {
    values.windows(2).all(|pair| key(&pair[0]) < key(&pair[1]))
}

fn valid_sorted_ids(values: &[String]) -> bool {
    values.iter().all(|value| valid_nonempty(value))
        && values.windows(2).all(|pair| pair[0] < pair[1])
}

fn valid_entity_relation_target(
    relation_id: &str,
    from_entity: &str,
    to_entity: &str,
    review_owner_ids: &[String],
) -> bool {
    valid_nonempty(relation_id)
        && valid_nonempty(from_entity)
        && valid_nonempty(to_entity)
        && from_entity != to_entity
        && strictly_sorted_unique(review_owner_ids)
        && review_owner_ids.iter().any(|owner| owner == from_entity)
        && review_owner_ids.iter().any(|owner| owner == to_entity)
}

fn valid_columns_and_rows(columns: &[String], rows: &[Vec<String>]) -> bool {
    valid_unique_nonempty(columns)
        && !rows.is_empty()
        && rows.iter().all(|row| row.len() == columns.len())
}

fn valid_unique_nonempty(values: &[String]) -> bool {
    use std::collections::BTreeSet;

    !values.is_empty()
        && values.iter().all(|value| valid_nonempty(value))
        && values.iter().collect::<BTreeSet<_>>().len() == values.len()
}

fn lower_hex_nibble(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

fn valid_embedding_hex(value: &str) -> bool {
    const EMBEDDING_BYTES: usize = 768 * std::mem::size_of::<f32>();
    if !is_lower_hex(value, EMBEDDING_BYTES * 2) {
        return false;
    }
    value.as_bytes().chunks_exact(8).all(|chunk| {
        let mut bytes = [0_u8; 4];
        for (index, pair) in chunk.chunks_exact(2).enumerate() {
            let Some(high) = lower_hex_nibble(pair[0]) else {
                return false;
            };
            let Some(low) = lower_hex_nibble(pair[1]) else {
                return false;
            };
            bytes[index] = (high << 4) | low;
        }
        f32::from_le_bytes(bytes).is_finite()
    })
}

fn valid_root_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    valid_nonempty(value)
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

fn parse_memory_type(value: &str) -> Result<MemoryType, RepairContractError> {
    if !MemoryType::all_values().contains(&value) {
        return Err(RepairContractError::InvalidMutation);
    }
    value
        .parse()
        .map_err(|_| RepairContractError::InvalidMutation)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(transparent)]
pub struct RepairDigest(String);

impl RepairDigest {
    pub fn parse(value: &str) -> Result<Self, RepairContractError> {
        if is_lower_hex(value, 64) {
            Ok(Self(value.to_string()))
        } else {
            Err(RepairContractError::InvalidDigest)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for RepairDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(&String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairLintScope {
    Global,
    Registered { space: String },
    Uncategorized,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairLintScopeWire {
    Global,
    Registered { space: String },
    Uncategorized,
}

impl RepairLintScope {
    pub const fn global() -> Self {
        Self::Global
    }

    pub fn registered(space: String) -> Result<Self, RepairContractError> {
        if valid_nonempty(&space) {
            Ok(Self::Registered { space })
        } else {
            Err(RepairContractError::InvalidSource)
        }
    }

    pub const fn uncategorized() -> Self {
        Self::Uncategorized
    }

    pub fn space(&self) -> Option<&str> {
        match self {
            Self::Registered { space } => Some(space),
            Self::Global | Self::Uncategorized => None,
        }
    }

    pub fn matches_report_scope_kind(&self, report_scope: &LintScope) -> bool {
        matches!(
            (self, report_scope.kind()),
            (Self::Global, LintScopeKind::Global)
                | (Self::Registered { .. }, LintScopeKind::Registered)
                | (Self::Uncategorized, LintScopeKind::Uncategorized)
        )
    }
}

impl<'de> Deserialize<'de> for RepairLintScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairLintScopeWire::deserialize(deserializer)? {
            RepairLintScopeWire::Global => Ok(Self::global()),
            RepairLintScopeWire::Registered { space } => Self::registered(space),
            RepairLintScopeWire::Uncategorized => Ok(Self::uncategorized()),
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairScope {
    Global,
    Registered { space: String },
    Uncategorized,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairScopeWire {
    Global,
    Registered { space: String },
    Uncategorized,
}

impl RepairScope {
    pub const fn global() -> Self {
        Self::Global
    }

    pub fn registered(space: String) -> Result<Self, RepairContractError> {
        if valid_nonempty(&space) {
            Ok(Self::Registered { space })
        } else {
            Err(RepairContractError::InvalidTarget)
        }
    }

    pub const fn uncategorized() -> Self {
        Self::Uncategorized
    }

    pub fn space(&self) -> Option<&str> {
        match self {
            Self::Registered { space } => Some(space),
            Self::Global | Self::Uncategorized => None,
        }
    }
}

impl<'de> Deserialize<'de> for RepairScope {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairScopeWire::deserialize(deserializer)? {
            RepairScopeWire::Global => Ok(Self::global()),
            RepairScopeWire::Registered { space } => Self::registered(space),
            RepairScopeWire::Uncategorized => Ok(Self::uncategorized()),
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairEnrichmentStep {
    EntityExtract,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairTarget {
    Memory {
        source_id: String,
        scope: RepairScope,
    },
    MemoryEntityLink {
        memory_id: String,
        entity_id: String,
        scope: RepairScope,
    },
    MemoryEntityExtraction {
        memory_id: String,
        step: RepairEnrichmentStep,
        entity_ids: Vec<String>,
        scope: RepairScope,
    },
    Tag {
        source: String,
        source_id: String,
        tag: String,
        scope: RepairScope,
    },
    PageLink {
        source_page_id: String,
        label_key: String,
        scope: RepairScope,
    },
    Page {
        page_id: String,
        scope: RepairScope,
    },
    PageProjection {
        page_id: String,
        scope: RepairScope,
    },
    EntityRelation {
        relation_id: String,
        from_entity: String,
        to_entity: String,
        review_owner_ids: Vec<String>,
        scope: RepairScope,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairTargetWire {
    Memory {
        source_id: String,
        scope: RepairScope,
    },
    MemoryEntityLink {
        memory_id: String,
        entity_id: String,
        scope: RepairScope,
    },
    MemoryEntityExtraction {
        memory_id: String,
        step: RepairEnrichmentStep,
        entity_ids: Vec<String>,
        scope: RepairScope,
    },
    Tag {
        source: String,
        source_id: String,
        tag: String,
        scope: RepairScope,
    },
    PageLink {
        source_page_id: String,
        label_key: String,
        scope: RepairScope,
    },
    Page {
        page_id: String,
        scope: RepairScope,
    },
    PageProjection {
        page_id: String,
        scope: RepairScope,
    },
    EntityRelation {
        relation_id: String,
        from_entity: String,
        to_entity: String,
        review_owner_ids: Vec<String>,
        scope: RepairScope,
    },
}

impl RepairTarget {
    pub fn memory(source_id: String, scope: RepairScope) -> Result<Self, RepairContractError> {
        if valid_nonempty(&source_id) {
            Ok(Self::Memory { source_id, scope })
        } else {
            Err(RepairContractError::InvalidTarget)
        }
    }

    pub fn memory_entity_link(
        memory_id: String,
        entity_id: String,
        scope: RepairScope,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&memory_id) || !valid_nonempty(&entity_id) {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::MemoryEntityLink {
            memory_id,
            entity_id,
            scope,
        })
    }

    pub fn memory_entity_extraction(
        memory_id: String,
        step: RepairEnrichmentStep,
        entity_ids: Vec<String>,
        scope: RepairScope,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&memory_id) || !valid_sorted_ids(&entity_ids) {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::MemoryEntityExtraction {
            memory_id,
            step,
            entity_ids,
            scope,
        })
    }

    pub fn tag(
        source: String,
        source_id: String,
        tag: String,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&source) || !valid_nonempty(&source_id) {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::Tag {
            source,
            source_id,
            tag,
            scope: RepairScope::global(),
        })
    }

    pub fn page_link(
        source_page_id: String,
        label_key: String,
        scope: RepairScope,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&source_page_id) || !valid_nonempty(&label_key) {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::PageLink {
            source_page_id,
            label_key,
            scope,
        })
    }

    pub fn page_projection(
        page_id: String,
        scope: RepairScope,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&page_id) {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::PageProjection { page_id, scope })
    }

    pub fn page(page_id: String, scope: RepairScope) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&page_id) {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::Page { page_id, scope })
    }

    pub fn entity_relation(
        relation_id: String,
        from_entity: String,
        to_entity: String,
        review_owner_ids: Vec<String>,
        scope: RepairScope,
    ) -> Result<Self, RepairContractError> {
        if !valid_entity_relation_target(&relation_id, &from_entity, &to_entity, &review_owner_ids)
        {
            return Err(RepairContractError::InvalidTarget);
        }
        Ok(Self::EntityRelation {
            relation_id,
            from_entity,
            to_entity,
            review_owner_ids,
            scope,
        })
    }

    pub fn memory_source_id(&self) -> &str {
        match self {
            Self::Memory { source_id, .. } => source_id,
            Self::MemoryEntityLink { .. }
            | Self::MemoryEntityExtraction { .. }
            | Self::Tag { .. }
            | Self::PageLink { .. }
            | Self::Page { .. }
            | Self::PageProjection { .. }
            | Self::EntityRelation { .. } => {
                panic!("repair target is not a memory")
            }
        }
    }

    pub fn scope(&self) -> &RepairScope {
        match self {
            Self::Memory { scope, .. }
            | Self::MemoryEntityLink { scope, .. }
            | Self::MemoryEntityExtraction { scope, .. }
            | Self::Tag { scope, .. }
            | Self::PageLink { scope, .. }
            | Self::Page { scope, .. }
            | Self::PageProjection { scope, .. }
            | Self::EntityRelation { scope, .. } => scope,
        }
    }

    pub fn review_owner_ids(&self) -> Option<Vec<String>> {
        match self {
            Self::Memory { source_id, .. } => Some(vec![source_id.clone()]),
            Self::PageProjection { page_id, .. } => Some(vec![page_id.clone()]),
            Self::MemoryEntityExtraction { memory_id, .. } => Some(vec![memory_id.clone()]),
            Self::EntityRelation {
                review_owner_ids, ..
            } => Some(review_owner_ids.clone()),
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for RepairTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairTargetWire::deserialize(deserializer)? {
            RepairTargetWire::Memory { source_id, scope } => Self::memory(source_id, scope),
            RepairTargetWire::MemoryEntityLink {
                memory_id,
                entity_id,
                scope,
            } => Self::memory_entity_link(memory_id, entity_id, scope),
            RepairTargetWire::MemoryEntityExtraction {
                memory_id,
                step,
                entity_ids,
                scope,
            } => Self::memory_entity_extraction(memory_id, step, entity_ids, scope),
            RepairTargetWire::Tag {
                source,
                source_id,
                tag,
                scope,
            } if scope == RepairScope::global() => Self::tag(source, source_id, tag),
            RepairTargetWire::Tag { .. } => Err(RepairContractError::InvalidTarget),
            RepairTargetWire::PageLink {
                source_page_id,
                label_key,
                scope,
            } => Self::page_link(source_page_id, label_key, scope),
            RepairTargetWire::Page { page_id, scope } => Self::page(page_id, scope),
            RepairTargetWire::PageProjection { page_id, scope } => {
                Self::page_projection(page_id, scope)
            }
            RepairTargetWire::EntityRelation {
                relation_id,
                from_entity,
                to_entity,
                review_owner_ids,
                scope,
            } => {
                Self::entity_relation(relation_id, from_entity, to_entity, review_owner_ids, scope)
            }
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairReviewBinding {
    review_id: String,
    occurrence_digest: RepairDigest,
    owner_ids: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairReviewBindingWire {
    review_id: String,
    occurrence_digest: RepairDigest,
    owner_ids: Vec<String>,
}

impl RepairReviewBinding {
    pub fn try_new(
        review_id: String,
        occurrence_digest: RepairDigest,
        owner_ids: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&review_id) || !strictly_sorted_unique(&owner_ids) {
            return Err(RepairContractError::InvalidSource);
        }
        Ok(Self {
            review_id,
            occurrence_digest,
            owner_ids,
        })
    }

    pub fn review_id(&self) -> &str {
        &self.review_id
    }

    pub fn occurrence_digest(&self) -> &RepairDigest {
        &self.occurrence_digest
    }

    pub fn owner_ids(&self) -> &[String] {
        &self.owner_ids
    }
}

impl<'de> Deserialize<'de> for RepairReviewBinding {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairReviewBindingWire::deserialize(deserializer)?;
        Self::try_new(wire.review_id, wire.occurrence_digest, wire.owner_ids)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairSource {
    report_schema_version: u16,
    check_catalog_version: u16,
    lint_scope: RepairLintScope,
    report_scope: LintScope,
    check_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    finding: Option<LintSemanticFinding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    deterministic_evidence: Vec<LintEvidenceRef>,
    general_snapshots: LintSnapshotReceipts,
    #[serde(skip_serializing_if = "Option::is_none")]
    deep_snapshots: Option<LintSnapshotReceipts>,
    general_producer_receipt: LintProducerReceipt,
    #[serde(skip_serializing_if = "Option::is_none")]
    deep_producer_receipt: Option<LintProducerReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_work_digest: Option<LintDigest>,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_binding: Option<RepairReviewBinding>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairSourceWire {
    report_schema_version: u16,
    check_catalog_version: u16,
    lint_scope: RepairLintScope,
    report_scope: LintScope,
    check_id: String,
    #[serde(default)]
    finding: Option<LintSemanticFinding>,
    #[serde(default)]
    deterministic_evidence: Vec<LintEvidenceRef>,
    general_snapshots: LintSnapshotReceipts,
    #[serde(default)]
    deep_snapshots: Option<LintSnapshotReceipts>,
    general_producer_receipt: LintProducerReceipt,
    #[serde(default)]
    deep_producer_receipt: Option<LintProducerReceipt>,
    #[serde(default)]
    agent_work_digest: Option<LintDigest>,
    #[serde(default)]
    review_binding: Option<RepairReviewBinding>,
}

impl RepairSource {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        lint_scope: RepairLintScope,
        report_scope: LintScope,
        finding: LintSemanticFinding,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: LintSnapshotReceipts,
        general_producer_receipt: LintProducerReceipt,
        deep_producer_receipt: LintProducerReceipt,
        agent_work_digest: LintDigest,
    ) -> Result<Self, RepairContractError> {
        if !lint_scope.matches_report_scope_kind(&report_scope)
            || finding.proposed_action() != LintSemanticAction::ReclassifyMemory
            || finding.unresolved_disagreement()
            || finding.evidence_ids().is_empty()
        {
            return Err(RepairContractError::InvalidSource);
        }
        Ok(Self {
            report_schema_version: LINT_REPORT_SCHEMA_VERSION,
            check_catalog_version: LINT_CHECK_CATALOG_VERSION,
            lint_scope,
            report_scope,
            check_id: REPAIR_CLASSIFICATION_CHECK_ID.to_string(),
            finding: Some(finding),
            deterministic_evidence: vec![],
            general_snapshots,
            deep_snapshots: Some(deep_snapshots),
            general_producer_receipt,
            deep_producer_receipt: Some(deep_producer_receipt),
            agent_work_digest: Some(agent_work_digest),
            review_binding: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_entity_relation(
        lint_scope: RepairLintScope,
        report_scope: LintScope,
        finding: LintSemanticFinding,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: LintSnapshotReceipts,
        general_producer_receipt: LintProducerReceipt,
        deep_producer_receipt: LintProducerReceipt,
        agent_work_digest: LintDigest,
    ) -> Result<Self, RepairContractError> {
        if !lint_scope.matches_report_scope_kind(&report_scope)
            || !matches!(
                finding.proposed_action(),
                LintSemanticAction::AddEntityRelation | LintSemanticAction::RemoveEntityRelation
            )
            || finding.unresolved_disagreement()
            || finding.evidence_ids().is_empty()
        {
            return Err(RepairContractError::InvalidSource);
        }
        Ok(Self {
            report_schema_version: LINT_REPORT_SCHEMA_VERSION,
            check_catalog_version: LINT_CHECK_CATALOG_VERSION,
            lint_scope,
            report_scope,
            check_id: REPAIR_RELATION_CHECK_ID.to_string(),
            finding: Some(finding),
            deterministic_evidence: vec![],
            general_snapshots,
            deep_snapshots: Some(deep_snapshots),
            general_producer_receipt,
            deep_producer_receipt: Some(deep_producer_receipt),
            agent_work_digest: Some(agent_work_digest),
            review_binding: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_deterministic(
        lint_scope: RepairLintScope,
        report_scope: LintScope,
        check_id: String,
        deterministic_evidence: Vec<LintEvidenceRef>,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: LintSnapshotReceipts,
        general_producer_receipt: LintProducerReceipt,
        deep_producer_receipt: LintProducerReceipt,
    ) -> Result<Self, RepairContractError> {
        if !lint_scope.matches_report_scope_kind(&report_scope)
            || !valid_nonempty(&check_id)
            || check_id == REPAIR_CLASSIFICATION_CHECK_ID
        {
            return Err(RepairContractError::InvalidSource);
        }
        Ok(Self {
            report_schema_version: LINT_REPORT_SCHEMA_VERSION,
            check_catalog_version: LINT_CHECK_CATALOG_VERSION,
            lint_scope,
            report_scope,
            check_id,
            finding: None,
            deterministic_evidence,
            general_snapshots,
            deep_snapshots: Some(deep_snapshots),
            general_producer_receipt,
            deep_producer_receipt: Some(deep_producer_receipt),
            agent_work_digest: None,
            review_binding: None,
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_general_only_deterministic(
        lint_scope: RepairLintScope,
        report_scope: LintScope,
        check_id: String,
        deterministic_evidence: Vec<LintEvidenceRef>,
        general_snapshots: LintSnapshotReceipts,
        general_producer_receipt: LintProducerReceipt,
    ) -> Result<Self, RepairContractError> {
        if !lint_scope.matches_report_scope_kind(&report_scope)
            || !valid_nonempty(&check_id)
            || check_id == REPAIR_CLASSIFICATION_CHECK_ID
        {
            return Err(RepairContractError::InvalidSource);
        }
        Ok(Self {
            report_schema_version: LINT_REPORT_SCHEMA_VERSION,
            check_catalog_version: LINT_CHECK_CATALOG_VERSION,
            lint_scope,
            report_scope,
            check_id,
            finding: None,
            deterministic_evidence,
            general_snapshots,
            deep_snapshots: None,
            general_producer_receipt,
            deep_producer_receipt: None,
            agent_work_digest: None,
            review_binding: None,
        })
    }

    pub fn lint_scope(&self) -> &RepairLintScope {
        &self.lint_scope
    }

    pub fn report_scope(&self) -> &LintScope {
        &self.report_scope
    }

    pub fn finding(&self) -> Option<&LintSemanticFinding> {
        self.finding.as_ref()
    }

    pub fn deterministic_evidence(&self) -> &[LintEvidenceRef] {
        &self.deterministic_evidence
    }

    pub fn check_id(&self) -> &str {
        &self.check_id
    }

    pub fn general_snapshots(&self) -> &LintSnapshotReceipts {
        &self.general_snapshots
    }

    pub fn deep_snapshots(&self) -> Option<&LintSnapshotReceipts> {
        self.deep_snapshots.as_ref()
    }

    pub fn general_producer_receipt(&self) -> &LintProducerReceipt {
        &self.general_producer_receipt
    }

    pub fn deep_producer_receipt(&self) -> Option<&LintProducerReceipt> {
        self.deep_producer_receipt.as_ref()
    }

    pub fn agent_work_digest(&self) -> Option<&LintDigest> {
        self.agent_work_digest.as_ref()
    }

    pub fn try_with_review_binding(
        mut self,
        review_binding: RepairReviewBinding,
    ) -> Result<Self, RepairContractError> {
        if self.review_binding.is_some() {
            return Err(RepairContractError::InvalidSource);
        }
        self.review_binding = Some(review_binding);
        Ok(self)
    }

    pub const fn review_binding(&self) -> Option<&RepairReviewBinding> {
        self.review_binding.as_ref()
    }

    pub const fn is_general_only_deterministic(&self) -> bool {
        self.finding.is_none()
            && self.agent_work_digest.is_none()
            && self.deep_snapshots.is_none()
            && self.deep_producer_receipt.is_none()
    }
}

impl<'de> Deserialize<'de> for RepairSource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairSourceWire::deserialize(deserializer)?;
        let report_schema_version = wire.report_schema_version;
        let check_catalog_version = wire.check_catalog_version;
        let review_binding = wire.review_binding;
        if (report_schema_version != PREVIOUS_LINT_REPORT_SCHEMA_VERSION
            && report_schema_version != LINT_REPORT_SCHEMA_VERSION)
            || (check_catalog_version != LEGACY_LINT_CHECK_CATALOG_VERSION
                && check_catalog_version != PREVIOUS_LINT_CHECK_CATALOG_VERSION
                && check_catalog_version != LINT_CHECK_CATALOG_VERSION)
            || wire.deep_snapshots.is_some() != wire.deep_producer_receipt.is_some()
        {
            return Err(D::Error::custom(RepairContractError::InvalidSource));
        }
        let mut source = if wire.check_id == REPAIR_CLASSIFICATION_CHECK_ID {
            if !wire.deterministic_evidence.is_empty()
                || wire.deep_snapshots.is_none()
                || wire.deep_producer_receipt.is_none()
            {
                return Err(D::Error::custom(RepairContractError::InvalidSource));
            }
            Self::try_new(
                wire.lint_scope,
                wire.report_scope,
                wire.finding
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
                wire.general_snapshots,
                wire.deep_snapshots
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
                wire.general_producer_receipt,
                wire.deep_producer_receipt
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
                wire.agent_work_digest
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
            )
        } else if wire.check_id == REPAIR_RELATION_CHECK_ID {
            if !wire.deterministic_evidence.is_empty()
                || wire.deep_snapshots.is_none()
                || wire.deep_producer_receipt.is_none()
            {
                return Err(D::Error::custom(RepairContractError::InvalidSource));
            }
            Self::try_new_entity_relation(
                wire.lint_scope,
                wire.report_scope,
                wire.finding
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
                wire.general_snapshots,
                wire.deep_snapshots
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
                wire.general_producer_receipt,
                wire.deep_producer_receipt
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
                wire.agent_work_digest
                    .ok_or_else(|| D::Error::custom(RepairContractError::InvalidSource))?,
            )
        } else {
            if wire.finding.is_some() || wire.agent_work_digest.is_some() {
                return Err(D::Error::custom(RepairContractError::InvalidSource));
            }
            match (wire.deep_snapshots, wire.deep_producer_receipt) {
                (Some(deep_snapshots), Some(deep_producer_receipt)) => Self::try_new_deterministic(
                    wire.lint_scope,
                    wire.report_scope,
                    wire.check_id,
                    wire.deterministic_evidence,
                    wire.general_snapshots,
                    deep_snapshots,
                    wire.general_producer_receipt,
                    deep_producer_receipt,
                ),
                (None, None) => Self::try_new_general_only_deterministic(
                    wire.lint_scope,
                    wire.report_scope,
                    wire.check_id,
                    wire.deterministic_evidence,
                    wire.general_snapshots,
                    wire.general_producer_receipt,
                ),
                _ => Err(RepairContractError::InvalidSource),
            }
        }
        .map_err(D::Error::custom)?;
        if let Some(review_binding) = review_binding {
            source = source
                .try_with_review_binding(review_binding)
                .map_err(D::Error::custom)?;
        }
        source.report_schema_version = report_schema_version;
        source.check_catalog_version = check_catalog_version;
        Ok(source)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairExpectedState {
    #[serde(skip_serializing_if = "Option::is_none")]
    version: Option<i64>,
    canonical_receipt: RepairDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairExpectedStateWire {
    version: Option<i64>,
    canonical_receipt: RepairDigest,
}

impl RepairExpectedState {
    pub fn try_new(
        version: Option<i64>,
        canonical_receipt: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        if version.is_some_and(|version| version < 0) {
            return Err(RepairContractError::InvalidExpectedState);
        }
        Ok(Self {
            version,
            canonical_receipt,
        })
    }

    pub const fn version(&self) -> Option<i64> {
        self.version
    }

    pub fn canonical_receipt(&self) -> &RepairDigest {
        &self.canonical_receipt
    }
}

impl<'de> Deserialize<'de> for RepairExpectedState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairExpectedStateWire::deserialize(deserializer)?;
        Self::try_new(wire.version, wire.canonical_receipt).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairWriter {
    ReclassifyMemory,
    RenamePageTitle,
    CompleteEntityExtraction,
    NormalizeMemorySourceAgent,
    ClearMemorySupersedes,
    UnstageOrphanRevision,
    DeleteTagRow,
    DeleteMemoryEntityLink,
    BindPageLink,
    ArchiveEmptySourcePage,
    RegeneratePageProjection,
    QuarantineStalePageProjection,
    EntityRelation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairMutation {
    ReclassifyMemory {
        before_memory_type: Option<MemoryType>,
        after_memory_type: MemoryType,
    },
    RenamePageTitle {
        before_title: String,
        after_title: String,
        after_embedding_hex: String,
    },
    CompleteEntityExtraction {
        entity_ids: Vec<String>,
    },
    NormalizeMemorySourceAgent {
        before_source_agent: String,
    },
    ClearMemorySupersedes {
        before_supersedes: String,
    },
    UnstageOrphanRevision,
    DeleteTagRow {
        source: String,
        source_id: String,
        tag: String,
    },
    DeleteMemoryEntityLink {
        memory_id: String,
        entity_id: String,
    },
    BindPageLink {
        before_target_page_id: Option<String>,
        after_target_page_id: String,
    },
    ArchiveEmptySourcePage {
        before_status: String,
        after_status: String,
    },
    RegeneratePageProjection {
        database_version: i64,
    },
    QuarantineStalePageProjection {
        source_path: String,
        quarantine_path: String,
    },
    EntityRelation {
        change: RepairRelationMutation,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairMutationWire {
    ReclassifyMemory {
        before_memory_type: Option<MemoryType>,
        after_memory_type: MemoryType,
    },
    RenamePageTitle {
        before_title: String,
        after_title: String,
        after_embedding_hex: String,
    },
    CompleteEntityExtraction {
        entity_ids: Vec<String>,
    },
    NormalizeMemorySourceAgent {
        before_source_agent: String,
    },
    ClearMemorySupersedes {
        before_supersedes: String,
    },
    UnstageOrphanRevision,
    DeleteTagRow {
        source: String,
        source_id: String,
        tag: String,
    },
    DeleteMemoryEntityLink {
        memory_id: String,
        entity_id: String,
    },
    BindPageLink {
        before_target_page_id: Option<String>,
        after_target_page_id: String,
    },
    ArchiveEmptySourcePage {
        before_status: String,
        after_status: String,
    },
    RegeneratePageProjection {
        database_version: i64,
    },
    QuarantineStalePageProjection {
        source_path: String,
        quarantine_path: String,
    },
    EntityRelation {
        change: RepairRelationMutation,
    },
}

impl RepairMutation {
    pub fn try_reclassify(
        before_memory_type: Option<&str>,
        after_memory_type: &str,
    ) -> Result<Self, RepairContractError> {
        let before_memory_type = before_memory_type.map(parse_memory_type).transpose()?;
        let after_memory_type = parse_memory_type(after_memory_type)?;
        Self::from_memory_types(before_memory_type, after_memory_type)
    }

    pub fn from_memory_types(
        before_memory_type: Option<MemoryType>,
        after_memory_type: MemoryType,
    ) -> Result<Self, RepairContractError> {
        if before_memory_type.as_ref() == Some(&after_memory_type) {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::ReclassifyMemory {
            before_memory_type,
            after_memory_type,
        })
    }

    pub fn normalize_memory_source_agent(
        before_source_agent: String,
    ) -> Result<Self, RepairContractError> {
        if before_source_agent.is_empty() || !before_source_agent.trim().is_empty() {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::NormalizeMemorySourceAgent {
            before_source_agent,
        })
    }

    pub fn rename_page_title(
        before_title: String,
        after_title: String,
        after_embedding_hex: String,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&before_title)
            || !valid_nonempty(&after_title)
            || before_title == after_title
            || !valid_embedding_hex(&after_embedding_hex)
        {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::RenamePageTitle {
            before_title,
            after_title,
            after_embedding_hex,
        })
    }

    pub fn complete_entity_extraction(
        entity_ids: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        if !valid_sorted_ids(&entity_ids) {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::CompleteEntityExtraction { entity_ids })
    }

    pub fn clear_memory_supersedes(before_supersedes: String) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&before_supersedes) {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::ClearMemorySupersedes { before_supersedes })
    }

    pub const fn unstage_orphan_revision() -> Self {
        Self::UnstageOrphanRevision
    }

    pub fn delete_tag_row(
        source: &str,
        source_id: &str,
        tag: &str,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(source) || !valid_nonempty(source_id) {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::DeleteTagRow {
            source: source.to_string(),
            source_id: source_id.to_string(),
            tag: tag.to_string(),
        })
    }

    pub fn delete_memory_entity_link(
        memory_id: &str,
        entity_id: &str,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(memory_id) || !valid_nonempty(entity_id) {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::DeleteMemoryEntityLink {
            memory_id: memory_id.to_string(),
            entity_id: entity_id.to_string(),
        })
    }

    pub fn bind_page_link(
        before_target_page_id: Option<String>,
        after_target_page_id: String,
    ) -> Result<Self, RepairContractError> {
        if before_target_page_id.is_some() || !valid_nonempty(&after_target_page_id) {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::BindPageLink {
            before_target_page_id,
            after_target_page_id,
        })
    }

    pub fn regenerate_page_projection(database_version: i64) -> Result<Self, RepairContractError> {
        if database_version < 0 {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::RegeneratePageProjection { database_version })
    }

    pub fn archive_empty_source_page() -> Self {
        Self::ArchiveEmptySourcePage {
            before_status: "active".to_string(),
            after_status: "archived".to_string(),
        }
    }

    pub fn quarantine_stale_page_projection(
        source_path: String,
        quarantine_path: String,
    ) -> Result<Self, RepairContractError> {
        if !valid_root_relative_path(&source_path)
            || source_path.starts_with(".wenlan/")
            || source_path.starts_with("_sources/")
            || !source_path.to_ascii_lowercase().ends_with(".md")
            || !valid_root_relative_path(&quarantine_path)
            || !quarantine_path.starts_with(".wenlan/orphaned/")
            || !quarantine_path.to_ascii_lowercase().ends_with(".md")
            || source_path == quarantine_path
        {
            return Err(RepairContractError::InvalidMutation);
        }
        Ok(Self::QuarantineStalePageProjection {
            source_path,
            quarantine_path,
        })
    }

    pub fn entity_relation(change: RepairRelationMutation) -> Result<Self, RepairContractError> {
        change
            .validate()
            .map_err(|_| RepairContractError::InvalidMutation)?;
        Ok(Self::EntityRelation { change })
    }

    pub fn before_memory_type(&self) -> Option<&str> {
        match self {
            Self::ReclassifyMemory {
                before_memory_type, ..
            } => before_memory_type.as_ref().map(|value| match value {
                MemoryType::Identity => "identity",
                MemoryType::Preference => "preference",
                MemoryType::Decision => "decision",
                MemoryType::Lesson => "lesson",
                MemoryType::Gotcha => "gotcha",
                MemoryType::Fact => "fact",
            }),
            Self::NormalizeMemorySourceAgent { .. }
            | Self::RenamePageTitle { .. }
            | Self::CompleteEntityExtraction { .. }
            | Self::ClearMemorySupersedes { .. }
            | Self::UnstageOrphanRevision
            | Self::DeleteTagRow { .. }
            | Self::DeleteMemoryEntityLink { .. }
            | Self::BindPageLink { .. }
            | Self::ArchiveEmptySourcePage { .. }
            | Self::RegeneratePageProjection { .. }
            | Self::QuarantineStalePageProjection { .. }
            | Self::EntityRelation { .. } => None,
        }
    }

    pub fn after_memory_type(&self) -> &str {
        match self {
            Self::ReclassifyMemory {
                after_memory_type, ..
            } => match after_memory_type {
                MemoryType::Identity => "identity",
                MemoryType::Preference => "preference",
                MemoryType::Decision => "decision",
                MemoryType::Lesson => "lesson",
                MemoryType::Gotcha => "gotcha",
                MemoryType::Fact => "fact",
            },
            Self::NormalizeMemorySourceAgent { .. }
            | Self::RenamePageTitle { .. }
            | Self::CompleteEntityExtraction { .. }
            | Self::ClearMemorySupersedes { .. }
            | Self::UnstageOrphanRevision
            | Self::DeleteTagRow { .. }
            | Self::DeleteMemoryEntityLink { .. }
            | Self::BindPageLink { .. }
            | Self::ArchiveEmptySourcePage { .. }
            | Self::RegeneratePageProjection { .. }
            | Self::QuarantineStalePageProjection { .. }
            | Self::EntityRelation { .. } => {
                panic!("repair mutation is not a memory reclassification")
            }
        }
    }
}

impl<'de> Deserialize<'de> for RepairMutation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairMutationWire::deserialize(deserializer)? {
            RepairMutationWire::ReclassifyMemory {
                before_memory_type,
                after_memory_type,
            } => Self::from_memory_types(before_memory_type, after_memory_type),
            RepairMutationWire::RenamePageTitle {
                before_title,
                after_title,
                after_embedding_hex,
            } => Self::rename_page_title(before_title, after_title, after_embedding_hex),
            RepairMutationWire::CompleteEntityExtraction { entity_ids } => {
                Self::complete_entity_extraction(entity_ids)
            }
            RepairMutationWire::NormalizeMemorySourceAgent {
                before_source_agent,
            } => Self::normalize_memory_source_agent(before_source_agent),
            RepairMutationWire::ClearMemorySupersedes { before_supersedes } => {
                Self::clear_memory_supersedes(before_supersedes)
            }
            RepairMutationWire::UnstageOrphanRevision => Ok(Self::unstage_orphan_revision()),
            RepairMutationWire::DeleteTagRow {
                source,
                source_id,
                tag,
            } => Self::delete_tag_row(&source, &source_id, &tag),
            RepairMutationWire::DeleteMemoryEntityLink {
                memory_id,
                entity_id,
            } => Self::delete_memory_entity_link(&memory_id, &entity_id),
            RepairMutationWire::BindPageLink {
                before_target_page_id,
                after_target_page_id,
            } => Self::bind_page_link(before_target_page_id, after_target_page_id),
            RepairMutationWire::ArchiveEmptySourcePage {
                before_status,
                after_status,
            } if before_status == "active" && after_status == "archived" => {
                Ok(Self::archive_empty_source_page())
            }
            RepairMutationWire::ArchiveEmptySourcePage { .. } => {
                Err(RepairContractError::InvalidMutation)
            }
            RepairMutationWire::RegeneratePageProjection { database_version } => {
                Self::regenerate_page_projection(database_version)
            }
            RepairMutationWire::QuarantineStalePageProjection {
                source_path,
                quarantine_path,
            } => Self::quarantine_stale_page_projection(source_path, quarantine_path),
            RepairMutationWire::EntityRelation { change } => Self::entity_relation(change),
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairMemoryField {
    MemoryType,
    SourceAgent,
    Supersedes,
    PendingRevision,
    TagRow,
    MemoryEntityLink,
    MemoryEntityLinks,
    EnrichmentStep,
    EntityEstablishment,
    TargetPageId,
    PageStatus,
    PageTitle,
    PageVersion,
    PageEmbedding,
    PageProjection,
    PageProjectionQuarantine,
    RelationEdges,
    CommunityGraphState,
    RelationVocabulary,
    RelationActivity,
    RelationReviewQueue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairAllowedEffects {
    owner: RepairTarget,
    fields: Vec<RepairMemoryField>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairAllowedEffectsWire {
    owner: RepairTarget,
    fields: Vec<RepairMemoryField>,
}

impl RepairAllowedEffects {
    pub fn memory_type(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::MemoryType],
        }
    }

    pub fn memory_source_agent(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::SourceAgent],
        }
    }

    pub fn memory_supersedes(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::Supersedes],
        }
    }

    pub fn memory_pending_revision(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::PendingRevision],
        }
    }

    pub fn tag_row(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::TagRow],
        }
    }

    pub fn memory_entity_link(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::MemoryEntityLink],
        }
    }

    pub fn complete_entity_extraction(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![
                RepairMemoryField::MemoryEntityLinks,
                RepairMemoryField::EnrichmentStep,
                // #708: recording the Nth memory link establishes the entity,
                // which writes `entity_confirmed` and `established_by` on its
                // page. The effect guard excludes that write, so the receipt
                // has to name it or it records an incomplete account.
                RepairMemoryField::EntityEstablishment,
            ],
        }
    }

    pub fn page_link_target(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::TargetPageId],
        }
    }

    pub fn page_projection(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::PageProjection],
        }
    }

    pub fn page_title_rename(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![
                RepairMemoryField::PageTitle,
                RepairMemoryField::PageVersion,
                RepairMemoryField::PageEmbedding,
                RepairMemoryField::PageProjection,
            ],
        }
    }

    pub fn page_status(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::PageStatus],
        }
    }

    pub fn page_projection_quarantine(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![RepairMemoryField::PageProjectionQuarantine],
        }
    }

    pub fn entity_relation(owner: RepairTarget) -> Self {
        Self {
            owner,
            fields: vec![
                RepairMemoryField::RelationEdges,
                RepairMemoryField::CommunityGraphState,
                RepairMemoryField::RelationVocabulary,
                RepairMemoryField::RelationActivity,
                RepairMemoryField::RelationReviewQueue,
            ],
        }
    }

    pub fn owner(&self) -> &RepairTarget {
        &self.owner
    }

    pub fn fields(&self) -> &[RepairMemoryField] {
        &self.fields
    }
}

impl<'de> Deserialize<'de> for RepairAllowedEffects {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairAllowedEffectsWire::deserialize(deserializer)?;
        let canonical = wire.fields.windows(2).all(|pair| pair[0] < pair[1]);
        let supported_shape = wire.fields.len() == 1
            || wire.fields
                == [
                    RepairMemoryField::MemoryEntityLinks,
                    RepairMemoryField::EnrichmentStep,
                ]
            // Since #708 the same writer also establishes the entity. The
            // two-field shape above stays supported for manifests prepared
            // before that upgrade.
            || wire.fields
                == [
                    RepairMemoryField::MemoryEntityLinks,
                    RepairMemoryField::EnrichmentStep,
                    RepairMemoryField::EntityEstablishment,
                ]
            || wire.fields
                == [
                    RepairMemoryField::PageTitle,
                    RepairMemoryField::PageVersion,
                    RepairMemoryField::PageEmbedding,
                    RepairMemoryField::PageProjection,
                ]
            || wire.fields
                == [
                    RepairMemoryField::RelationEdges,
                    RepairMemoryField::CommunityGraphState,
                    RepairMemoryField::RelationVocabulary,
                    RepairMemoryField::RelationActivity,
                    RepairMemoryField::RelationReviewQueue,
                ];
        if !canonical || !supported_shape {
            return Err(D::Error::custom(RepairContractError::InvalidAllowedEffects));
        }
        Ok(Self {
            owner: wire.owner,
            fields: wire.fields,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairRollbackArtifact {
    format_version: u16,
    relative_path: String,
    digest: RepairDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRollbackArtifactWire {
    format_version: u16,
    relative_path: String,
    digest: RepairDigest,
}

impl RepairRollbackArtifact {
    pub fn try_new(
        relative_path: String,
        digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_for_format(
            PREVIOUS_REPAIR_ROLLBACK_FORMAT_VERSION,
            relative_path,
            digest,
        )
    }

    pub fn try_new_v2(
        relative_path: String,
        digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_for_format(REPAIR_ROLLBACK_FORMAT_VERSION, relative_path, digest)
    }

    pub fn entity_relation(
        relative_path: String,
        digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_for_format(
            REPAIR_RELATION_ROLLBACK_FORMAT_VERSION,
            relative_path,
            digest,
        )
    }

    fn try_new_for_format(
        format_version: u16,
        relative_path: String,
        digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        let path = Path::new(&relative_path);
        if !matches!(
            format_version,
            1 | REPAIR_ROLLBACK_FORMAT_VERSION | REPAIR_RELATION_ROLLBACK_FORMAT_VERSION
        ) || !valid_nonempty(&relative_path)
            || path.is_absolute()
            || path
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(RepairContractError::InvalidRollbackArtifact);
        }
        Ok(Self {
            format_version,
            relative_path,
            digest,
        })
    }

    pub const fn format_version(&self) -> u16 {
        self.format_version
    }

    pub fn relative_path(&self) -> &str {
        &self.relative_path
    }

    pub fn digest(&self) -> &RepairDigest {
        &self.digest
    }
}

impl<'de> Deserialize<'de> for RepairRollbackArtifact {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairRollbackArtifactWire::deserialize(deserializer)?;
        Self::try_new_for_format(wire.format_version, wire.relative_path, wire.digest)
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairVerificationPolicy {
    LegacyWholeReports,
    ApplicableChecks {
        required_deep_check_ids: Vec<String>,
    },
    GeneralOnly,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairVerificationPolicyWire {
    LegacyWholeReports,
    ApplicableChecks {
        required_deep_check_ids: Vec<String>,
    },
    GeneralOnly,
}

impl RepairVerificationPolicy {
    fn applicable_checks(
        required_deep_check_ids: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        if required_deep_check_ids
            .iter()
            .any(|check_id| !valid_nonempty(check_id))
            || !required_deep_check_ids
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        {
            return Err(RepairContractError::InvalidPostAssertions);
        }
        Ok(Self::ApplicableChecks {
            required_deep_check_ids,
        })
    }

    pub fn required_deep_check_ids(&self) -> Option<&[String]> {
        match self {
            Self::LegacyWholeReports | Self::GeneralOnly => None,
            Self::ApplicableChecks {
                required_deep_check_ids,
            } => Some(required_deep_check_ids),
        }
    }

    pub const fn requires_whole_reports(&self) -> bool {
        matches!(self, Self::LegacyWholeReports)
    }

    pub const fn is_general_only(&self) -> bool {
        matches!(self, Self::GeneralOnly)
    }
}

impl<'de> Deserialize<'de> for RepairVerificationPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairVerificationPolicyWire::deserialize(deserializer)?;
        match wire {
            RepairVerificationPolicyWire::LegacyWholeReports => Ok(Self::LegacyWholeReports),
            RepairVerificationPolicyWire::ApplicableChecks {
                required_deep_check_ids,
            } => Self::applicable_checks(required_deep_check_ids).map_err(D::Error::custom),
            RepairVerificationPolicyWire::GeneralOnly => Ok(Self::GeneralOnly),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairPostAssertions {
    target_check_id: String,
    target_evidence_id: LintDigest,
    general_baseline: Vec<RepairCheckBaseline>,
    deep_baseline: Vec<RepairCheckBaseline>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_record_set: Option<RepairRecordSetBaseline>,
    verification_policy: RepairVerificationPolicy,
    require_complete_general: bool,
    reject_new_actionable: bool,
    reject_new_incomplete: bool,
    allowed_non_target_check_deltas: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairPostAssertionsWire {
    target_check_id: String,
    target_evidence_id: LintDigest,
    general_baseline: Vec<RepairCheckBaseline>,
    deep_baseline: Vec<RepairCheckBaseline>,
    #[serde(default)]
    target_record_set: Option<RepairRecordSetBaseline>,
    verification_policy: RepairVerificationPolicy,
    require_complete_general: bool,
    reject_new_actionable: bool,
    reject_new_incomplete: bool,
    allowed_non_target_check_deltas: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairCheckBaseline {
    check_id: String,
    outcome: crate::lint::LintOutcome,
    gate_effect: crate::lint::LintGateEffect,
    #[serde(skip_serializing_if = "Option::is_none")]
    affected_records: Option<u64>,
    evidence: Vec<crate::lint::LintEvidenceRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairCheckBaselineWire {
    check_id: String,
    outcome: crate::lint::LintOutcome,
    gate_effect: crate::lint::LintGateEffect,
    #[serde(default)]
    affected_records: Option<u64>,
    evidence: Vec<crate::lint::LintEvidenceRef>,
}

impl RepairCheckBaseline {
    pub fn try_new(
        check_id: String,
        outcome: crate::lint::LintOutcome,
        gate_effect: crate::lint::LintGateEffect,
        evidence: Vec<crate::lint::LintEvidenceRef>,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_with_affected_records(check_id, outcome, gate_effect, None, evidence)
    }

    pub fn try_new_current(
        check_id: String,
        outcome: crate::lint::LintOutcome,
        gate_effect: crate::lint::LintGateEffect,
        affected_records: u64,
        evidence: Vec<crate::lint::LintEvidenceRef>,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_with_affected_records(
            check_id,
            outcome,
            gate_effect,
            Some(affected_records),
            evidence,
        )
    }

    fn try_new_with_affected_records(
        check_id: String,
        outcome: crate::lint::LintOutcome,
        gate_effect: crate::lint::LintGateEffect,
        affected_records: Option<u64>,
        evidence: Vec<crate::lint::LintEvidenceRef>,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&check_id) {
            return Err(RepairContractError::InvalidPostAssertions);
        }
        Ok(Self {
            check_id,
            outcome,
            gate_effect,
            affected_records,
            evidence,
        })
    }

    pub fn check_id(&self) -> &str {
        &self.check_id
    }

    pub const fn outcome(&self) -> crate::lint::LintOutcome {
        self.outcome
    }

    pub const fn gate_effect(&self) -> crate::lint::LintGateEffect {
        self.gate_effect
    }

    pub const fn affected_records(&self) -> Option<u64> {
        self.affected_records
    }

    pub fn evidence(&self) -> &[crate::lint::LintEvidenceRef] {
        &self.evidence
    }
}

impl<'de> Deserialize<'de> for RepairCheckBaseline {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairCheckBaselineWire::deserialize(deserializer)?;
        Self::try_new_with_affected_records(
            wire.check_id,
            wire.outcome,
            wire.gate_effect,
            wire.affected_records,
            wire.evidence,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairRecordSetBaseline {
    record_count: u64,
    digest: RepairDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairRecordSetBaselineWire {
    record_count: u64,
    digest: RepairDigest,
}

impl RepairRecordSetBaseline {
    pub fn try_new(record_count: u64, digest: RepairDigest) -> Result<Self, RepairContractError> {
        if record_count == 0 {
            return Err(RepairContractError::InvalidPostAssertions);
        }
        Ok(Self {
            record_count,
            digest,
        })
    }

    pub const fn record_count(&self) -> u64 {
        self.record_count
    }

    pub const fn digest(&self) -> &RepairDigest {
        &self.digest
    }
}

impl<'de> Deserialize<'de> for RepairRecordSetBaseline {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairRecordSetBaselineWire::deserialize(deserializer)?;
        Self::try_new(wire.record_count, wire.digest).map_err(D::Error::custom)
    }
}

fn valid_check_baseline(values: &[RepairCheckBaseline]) -> bool {
    !values.is_empty()
        && values
            .windows(2)
            .all(|pair| pair[0].check_id() < pair[1].check_id())
}

fn manifest_baseline_schema_matches(
    manifest_schema_version: u16,
    assertions: &RepairPostAssertions,
) -> bool {
    let baselines = assertions
        .general_baseline()
        .iter()
        .chain(assertions.deep_baseline());
    match manifest_schema_version {
        2 => {
            assertions.target_record_set().is_none()
                && baselines
                    .clone()
                    .all(|baseline| baseline.affected_records().is_none())
                && !baselines
                    .flat_map(RepairCheckBaseline::evidence)
                    .any(|evidence| matches!(evidence, LintEvidenceRef::OpaqueDigest { .. }))
        }
        3 => assertions.target_record_set().is_none(),
        4 | 5 | PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION | REPAIR_MANIFEST_SCHEMA_VERSION
            if assertions.target_check_id() == REPAIR_TAG_INTEGRITY_CHECK_ID =>
        {
            assertions.target_record_set().is_some()
                && assertions
                    .general_baseline()
                    .iter()
                    .find(|baseline| baseline.check_id() == REPAIR_TAG_INTEGRITY_CHECK_ID)
                    .is_some_and(|baseline| baseline.affected_records().is_some())
        }
        4 | 5 | PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION | REPAIR_MANIFEST_SCHEMA_VERSION => {
            assertions.target_record_set().is_none()
        }
        _ => false,
    }
}

impl RepairPostAssertions {
    pub fn try_new(
        target_evidence_id: LintDigest,
        general_baseline: Vec<RepairCheckBaseline>,
        deep_baseline: Vec<RepairCheckBaseline>,
        allowed_non_target_check_deltas: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_for_check(
            REPAIR_CLASSIFICATION_CHECK_ID.to_string(),
            target_evidence_id,
            general_baseline,
            deep_baseline,
            vec![REPAIR_CLASSIFICATION_CHECK_ID.to_string()],
            allowed_non_target_check_deltas,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_for_check(
        target_check_id: String,
        target_evidence_id: LintDigest,
        general_baseline: Vec<RepairCheckBaseline>,
        deep_baseline: Vec<RepairCheckBaseline>,
        required_deep_check_ids: Vec<String>,
        allowed_non_target_check_deltas: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&target_check_id)
            || !valid_check_baseline(&general_baseline)
            || !valid_check_baseline(&deep_baseline)
            || allowed_non_target_check_deltas
                .iter()
                .any(|value| !valid_nonempty(value))
            || !allowed_non_target_check_deltas
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        {
            return Err(RepairContractError::InvalidPostAssertions);
        }
        Ok(Self {
            target_check_id,
            target_evidence_id,
            general_baseline,
            deep_baseline,
            target_record_set: None,
            verification_policy: RepairVerificationPolicy::applicable_checks(
                required_deep_check_ids,
            )?,
            require_complete_general: true,
            reject_new_actionable: true,
            reject_new_incomplete: true,
            allowed_non_target_check_deltas,
        })
    }

    pub fn try_new_general_only_for_check(
        target_check_id: String,
        target_evidence_id: LintDigest,
        general_baseline: Vec<RepairCheckBaseline>,
        allowed_non_target_check_deltas: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&target_check_id)
            || !valid_check_baseline(&general_baseline)
            || allowed_non_target_check_deltas
                .iter()
                .any(|value| !valid_nonempty(value))
            || !allowed_non_target_check_deltas
                .windows(2)
                .all(|pair| pair[0] < pair[1])
        {
            return Err(RepairContractError::InvalidPostAssertions);
        }
        Ok(Self {
            target_check_id,
            target_evidence_id,
            general_baseline,
            deep_baseline: Vec::new(),
            target_record_set: None,
            verification_policy: RepairVerificationPolicy::GeneralOnly,
            require_complete_general: true,
            reject_new_actionable: true,
            reject_new_incomplete: true,
            allowed_non_target_check_deltas,
        })
    }

    fn try_new_legacy_v1(
        target_evidence_id: LintDigest,
        general_baseline: Vec<RepairCheckBaseline>,
        deep_baseline: Vec<RepairCheckBaseline>,
        allowed_non_target_check_deltas: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        let mut assertions = Self::try_new(
            target_evidence_id,
            general_baseline,
            deep_baseline,
            allowed_non_target_check_deltas,
        )?;
        assertions.verification_policy = RepairVerificationPolicy::LegacyWholeReports;
        Ok(assertions)
    }

    pub fn target_evidence_id(&self) -> &LintDigest {
        &self.target_evidence_id
    }

    pub fn target_check_id(&self) -> &str {
        &self.target_check_id
    }

    pub fn try_with_target_record_set(
        mut self,
        target_record_set: RepairRecordSetBaseline,
    ) -> Result<Self, RepairContractError> {
        if self.target_check_id != REPAIR_TAG_INTEGRITY_CHECK_ID || self.target_record_set.is_some()
        {
            return Err(RepairContractError::InvalidPostAssertions);
        }
        self.target_record_set = Some(target_record_set);
        Ok(self)
    }

    pub fn general_baseline(&self) -> &[RepairCheckBaseline] {
        &self.general_baseline
    }

    pub fn deep_baseline(&self) -> &[RepairCheckBaseline] {
        &self.deep_baseline
    }

    pub const fn target_record_set(&self) -> Option<&RepairRecordSetBaseline> {
        self.target_record_set.as_ref()
    }

    pub const fn verification_policy(&self) -> &RepairVerificationPolicy {
        &self.verification_policy
    }

    pub fn allowed_non_target_check_deltas(&self) -> &[String] {
        &self.allowed_non_target_check_deltas
    }
}

impl<'de> Deserialize<'de> for RepairPostAssertions {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairPostAssertionsWire::deserialize(deserializer)?;
        if !wire.require_complete_general
            || !wire.reject_new_actionable
            || !wire.reject_new_incomplete
            || wire.verification_policy.requires_whole_reports()
        {
            return Err(D::Error::custom(RepairContractError::InvalidPostAssertions));
        }
        if wire.verification_policy.is_general_only() {
            if !wire.deep_baseline.is_empty() {
                return Err(D::Error::custom(RepairContractError::InvalidPostAssertions));
            }
            let assertions = Self::try_new_general_only_for_check(
                wire.target_check_id,
                wire.target_evidence_id,
                wire.general_baseline,
                wire.allowed_non_target_check_deltas,
            )
            .map_err(D::Error::custom)?;
            return match wire.target_record_set {
                Some(target_record_set) => assertions
                    .try_with_target_record_set(target_record_set)
                    .map_err(D::Error::custom),
                None => Ok(assertions),
            };
        }
        let assertions = Self::try_new_for_check(
            wire.target_check_id,
            wire.target_evidence_id,
            wire.general_baseline,
            wire.deep_baseline,
            wire.verification_policy
                .required_deep_check_ids()
                .unwrap_or_default()
                .to_vec(),
            wire.allowed_non_target_check_deltas,
        )
        .map_err(D::Error::custom)?;
        match wire.target_record_set {
            Some(target_record_set) => assertions
                .try_with_target_record_set(target_record_set)
                .map_err(D::Error::custom),
            None => Ok(assertions),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairManifestDraft {
    manifest_schema_version: u16,
    manifest_id: String,
    prepared_at: i64,
    source: RepairSource,
    target: RepairTarget,
    expected_state: RepairExpectedState,
    writer: RepairWriter,
    mutation: RepairMutation,
    allowed_effects: RepairAllowedEffects,
    rollback: RepairRollbackArtifact,
    post_assertions: RepairPostAssertions,
}

impl RepairManifestDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        manifest_id: String,
        prepared_at: i64,
        source: RepairSource,
        target: RepairTarget,
        expected_state: RepairExpectedState,
        writer: RepairWriter,
        mutation: RepairMutation,
        allowed_effects: RepairAllowedEffects,
        rollback: RepairRollbackArtifact,
        post_assertions: RepairPostAssertions,
    ) -> Result<Self, RepairContractError> {
        let manifest_schema_version = match writer {
            RepairWriter::EntityRelation => REPAIR_MANIFEST_SCHEMA_VERSION,
            RepairWriter::RenamePageTitle | RepairWriter::CompleteEntityExtraction => {
                PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION
            }
            RepairWriter::ReclassifyMemory if source.review_binding().is_some() => {
                PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION
            }
            _ => LEGACY_REPAIR_MANIFEST_SCHEMA_VERSION,
        };
        Self::try_new_for_schema(
            manifest_schema_version,
            manifest_id,
            prepared_at,
            source,
            target,
            expected_state,
            writer,
            mutation,
            allowed_effects,
            rollback,
            post_assertions,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn try_new_for_schema(
        manifest_schema_version: u16,
        manifest_id: String,
        prepared_at: i64,
        source: RepairSource,
        target: RepairTarget,
        expected_state: RepairExpectedState,
        writer: RepairWriter,
        mutation: RepairMutation,
        allowed_effects: RepairAllowedEffects,
        rollback: RepairRollbackArtifact,
        post_assertions: RepairPostAssertions,
    ) -> Result<Self, RepairContractError> {
        if !matches!(
            manifest_schema_version,
            2 | 3
                | 4
                | 5
                | PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION
                | REPAIR_MANIFEST_SCHEMA_VERSION
        ) {
            return Err(RepairContractError::UnsupportedManifestSchema);
        }
        let expected_report_schema_version = if manifest_schema_version == 2 {
            PREVIOUS_LINT_REPORT_SCHEMA_VERSION
        } else {
            LINT_REPORT_SCHEMA_VERSION
        };
        let catalog_version_supported = source.check_catalog_version == LINT_CHECK_CATALOG_VERSION
            || source.check_catalog_version == PREVIOUS_LINT_CHECK_CATALOG_VERSION
            || (matches!(manifest_schema_version, 2..=4)
                && source.check_catalog_version == LEGACY_LINT_CHECK_CATALOG_VERSION);
        if source.report_schema_version != expected_report_schema_version
            || !catalog_version_supported
            || !manifest_baseline_schema_matches(manifest_schema_version, &post_assertions)
        {
            return Err(RepairContractError::InvalidManifest);
        }
        if manifest_schema_version == 2
            && (source.is_general_only_deterministic()
                || source
                    .deterministic_evidence()
                    .iter()
                    .any(|evidence| matches!(evidence, LintEvidenceRef::OpaqueDigest { .. }))
                || post_assertions.verification_policy().is_general_only())
        {
            return Err(RepairContractError::InvalidManifest);
        }
        if !valid_manifest_id(&manifest_id) || prepared_at <= 0 {
            return Err(RepairContractError::InvalidManifest);
        }
        let compatible = match (&target, writer, &mutation, allowed_effects.fields()) {
            (
                RepairTarget::EntityRelation {
                    relation_id,
                    from_entity,
                    to_entity,
                    review_owner_ids,
                    ..
                },
                RepairWriter::EntityRelation,
                RepairMutation::EntityRelation { change },
                [RepairMemoryField::RelationEdges, RepairMemoryField::CommunityGraphState, RepairMemoryField::RelationVocabulary, RepairMemoryField::RelationActivity, RepairMemoryField::RelationReviewQueue],
            ) => {
                valid_entity_relation_target(relation_id, from_entity, to_entity, review_owner_ids)
                    && change.validate().is_ok()
            }
            (
                RepairTarget::Memory { .. },
                RepairWriter::ReclassifyMemory,
                RepairMutation::ReclassifyMemory { .. },
                [RepairMemoryField::MemoryType],
            )
            | (
                RepairTarget::PageProjection { .. },
                RepairWriter::RenamePageTitle,
                RepairMutation::RenamePageTitle { .. },
                [RepairMemoryField::PageTitle, RepairMemoryField::PageVersion, RepairMemoryField::PageEmbedding, RepairMemoryField::PageProjection],
            )
            | (
                RepairTarget::Memory { .. },
                RepairWriter::NormalizeMemorySourceAgent,
                RepairMutation::NormalizeMemorySourceAgent { .. },
                [RepairMemoryField::SourceAgent],
            )
            | (
                RepairTarget::Memory { .. },
                RepairWriter::ClearMemorySupersedes,
                RepairMutation::ClearMemorySupersedes { .. },
                [RepairMemoryField::Supersedes],
            )
            | (
                RepairTarget::Memory { .. },
                RepairWriter::UnstageOrphanRevision,
                RepairMutation::UnstageOrphanRevision,
                [RepairMemoryField::PendingRevision],
            )
            | (
                RepairTarget::Tag { .. },
                RepairWriter::DeleteTagRow,
                RepairMutation::DeleteTagRow { .. },
                [RepairMemoryField::TagRow],
            )
            | (
                RepairTarget::PageLink { .. },
                RepairWriter::BindPageLink,
                RepairMutation::BindPageLink { .. },
                [RepairMemoryField::TargetPageId],
            )
            | (
                RepairTarget::PageProjection { .. },
                RepairWriter::RegeneratePageProjection,
                RepairMutation::RegeneratePageProjection { .. },
                [RepairMemoryField::PageProjection],
            )
            | (
                RepairTarget::PageProjection { .. },
                RepairWriter::QuarantineStalePageProjection,
                RepairMutation::QuarantineStalePageProjection { .. },
                [RepairMemoryField::PageProjectionQuarantine],
            ) => true,
            (
                RepairTarget::Page { .. },
                RepairWriter::ArchiveEmptySourcePage,
                RepairMutation::ArchiveEmptySourcePage {
                    before_status,
                    after_status,
                },
                [RepairMemoryField::PageStatus],
            ) if before_status == "active" && after_status == "archived" => true,
            (
                RepairTarget::MemoryEntityLink {
                    memory_id,
                    entity_id,
                    ..
                },
                RepairWriter::DeleteMemoryEntityLink,
                RepairMutation::DeleteMemoryEntityLink {
                    memory_id: mutation_memory_id,
                    entity_id: mutation_entity_id,
                },
                [RepairMemoryField::MemoryEntityLink],
            ) => memory_id == mutation_memory_id && entity_id == mutation_entity_id,
            (
                RepairTarget::MemoryEntityExtraction {
                    step: RepairEnrichmentStep::EntityExtract,
                    entity_ids,
                    ..
                },
                RepairWriter::CompleteEntityExtraction,
                RepairMutation::CompleteEntityExtraction {
                    entity_ids: mutation_entity_ids,
                },
                // Three fields is what this writer declares since #708, when
                // recording the Nth memory link began establishing the entity
                // too. The two-field form stays valid so a manifest prepared
                // before that upgrade can still be applied and verified.
                [RepairMemoryField::MemoryEntityLinks, RepairMemoryField::EnrichmentStep]
                | [RepairMemoryField::MemoryEntityLinks, RepairMemoryField::EnrichmentStep, RepairMemoryField::EntityEstablishment],
            ) => entity_ids == mutation_entity_ids,
            _ => false,
        };
        if !compatible {
            return Err(RepairContractError::UnsupportedWriter);
        }
        let minimum_schema = match writer {
            RepairWriter::EntityRelation => REPAIR_MANIFEST_SCHEMA_VERSION,
            RepairWriter::RenamePageTitle | RepairWriter::CompleteEntityExtraction => {
                PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION
            }
            RepairWriter::UnstageOrphanRevision
            | RepairWriter::DeleteMemoryEntityLink
            | RepairWriter::ArchiveEmptySourcePage
            | RepairWriter::QuarantineStalePageProjection => LEGACY_REPAIR_MANIFEST_SCHEMA_VERSION,
            _ => 2,
        };
        let aggregate_writer = matches!(
            writer,
            RepairWriter::RenamePageTitle | RepairWriter::CompleteEntityExtraction
        );
        let relation_writer = writer == RepairWriter::EntityRelation;
        let current_semantic_review_writer = writer == RepairWriter::ReclassifyMemory
            && manifest_schema_version == PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION;
        if manifest_schema_version < minimum_schema
            || (relation_writer
                && (manifest_schema_version != REPAIR_MANIFEST_SCHEMA_VERSION
                    || rollback.format_version() != REPAIR_RELATION_ROLLBACK_FORMAT_VERSION))
            || (aggregate_writer
                && (manifest_schema_version != PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION
                    || rollback.format_version() != REPAIR_ROLLBACK_FORMAT_VERSION))
            || (current_semantic_review_writer
                && rollback.format_version() != PREVIOUS_REPAIR_ROLLBACK_FORMAT_VERSION)
            || (!aggregate_writer
                && !relation_writer
                && !current_semantic_review_writer
                && (manifest_schema_version > LEGACY_REPAIR_MANIFEST_SCHEMA_VERSION
                    || rollback.format_version() != PREVIOUS_REPAIR_ROLLBACK_FORMAT_VERSION))
        {
            return Err(RepairContractError::UnsupportedWriter);
        }
        let expected_review_owner_ids = target.review_owner_ids();
        let review_binding_valid = match writer {
            RepairWriter::EntityRelation
            | RepairWriter::RenamePageTitle
            | RepairWriter::CompleteEntityExtraction => source
                .review_binding()
                .zip(expected_review_owner_ids.as_ref())
                .is_some_and(|(binding, owner_ids)| binding.owner_ids() == owner_ids),
            RepairWriter::ReclassifyMemory
                if manifest_schema_version == PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION =>
            {
                source
                    .review_binding()
                    .zip(expected_review_owner_ids.as_ref())
                    .is_some_and(|(binding, owner_ids)| binding.owner_ids() == owner_ids)
            }
            _ => source.review_binding().is_none(),
        };
        let relation_source_valid = if writer == RepairWriter::EntityRelation {
            match (&target, &mutation, source.finding()) {
                (
                    RepairTarget::EntityRelation {
                        relation_id,
                        review_owner_ids,
                        ..
                    },
                    RepairMutation::EntityRelation {
                        change:
                            RepairRelationMutation::Add {
                                source_memory_id,
                                confidence_basis_points,
                                retire_relation_ids,
                                ..
                            },
                    },
                    Some(finding),
                ) => {
                    finding.proposed_action() == LintSemanticAction::AddEntityRelation
                        && !finding.unresolved_disagreement()
                        && finding.confidence_basis_points() == *confidence_basis_points
                        && source_memory_id
                            .as_ref()
                            .is_none_or(|source_id| review_owner_ids.contains(source_id))
                        && !retire_relation_ids.contains(relation_id)
                }
                (
                    RepairTarget::EntityRelation { .. },
                    RepairMutation::EntityRelation {
                        change: RepairRelationMutation::Retire,
                    },
                    Some(finding),
                ) => {
                    finding.proposed_action() == LintSemanticAction::RemoveEntityRelation
                        && !finding.unresolved_disagreement()
                }
                _ => false,
            }
        } else {
            true
        };
        if allowed_effects.owner() != &target
            || source.check_id() != post_assertions.target_check_id()
            || !repair_writer_authorized_for_source(&source, writer)
            || source.is_general_only_deterministic()
                != post_assertions.verification_policy().is_general_only()
            || !review_binding_valid
            || !relation_source_valid
            || source.finding().is_some_and(|finding| {
                !finding
                    .evidence_ids()
                    .contains(post_assertions.target_evidence_id())
            })
        {
            return Err(RepairContractError::InvalidManifest);
        }
        Ok(Self {
            manifest_schema_version,
            manifest_id,
            prepared_at,
            source,
            target,
            expected_state,
            writer,
            mutation,
            allowed_effects,
            rollback,
            post_assertions,
        })
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

fn repair_writer_authorized_for_source(source: &RepairSource, writer: RepairWriter) -> bool {
    if source.finding().is_some() {
        return (source.check_id() == REPAIR_CLASSIFICATION_CHECK_ID
            && writer == RepairWriter::ReclassifyMemory)
            || (source.check_id() == REPAIR_RELATION_CHECK_ID
                && writer == RepairWriter::EntityRelation);
    }
    matches!(
        (source.check_id(), writer),
        (
            REPAIR_MEMORY_STATE_CHECK_ID,
            RepairWriter::NormalizeMemorySourceAgent
                | RepairWriter::ClearMemorySupersedes
                | RepairWriter::UnstageOrphanRevision
        ) | (
            REPAIR_SUPERSESSION_CHECK_ID,
            RepairWriter::ClearMemorySupersedes | RepairWriter::UnstageOrphanRevision
        ) | (REPAIR_TAG_INTEGRITY_CHECK_ID, RepairWriter::DeleteTagRow)
            | (
                REPAIR_MEMORY_ENTITY_INTEGRITY_CHECK_ID,
                RepairWriter::DeleteMemoryEntityLink
            )
            | (REPAIR_ORPHAN_LABELS_CHECK_ID, RepairWriter::BindPageLink)
            | (
                REPAIR_PROJECTION_VERSION_CHECK_ID,
                RepairWriter::RegeneratePageProjection
            )
            | (
                REPAIR_PROJECTION_IDENTITY_CHECK_ID,
                RepairWriter::QuarantineStalePageProjection
            )
            | (
                REPAIR_SOURCE_PAGE_INTEGRITY_CHECK_ID,
                RepairWriter::ArchiveEmptySourcePage
            )
            | (
                "pages.duplicate_active_titles",
                RepairWriter::RenamePageTitle
            )
            | (
                "memories.enrichment_failures",
                RepairWriter::CompleteEntityExtraction
            )
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairManifest {
    #[serde(flatten)]
    draft: RepairManifestDraft,
    manifest_digest: RepairDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairManifestWire {
    manifest_schema_version: u16,
    manifest_id: String,
    prepared_at: i64,
    source: RepairSource,
    target: RepairTarget,
    expected_state: RepairExpectedState,
    writer: RepairWriter,
    mutation: RepairMutation,
    allowed_effects: RepairAllowedEffects,
    rollback: RepairRollbackArtifact,
    post_assertions: RepairPostAssertions,
    manifest_digest: RepairDigest,
}

impl RepairManifest {
    pub fn try_new(
        draft: RepairManifestDraft,
        manifest_digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        Ok(Self {
            draft,
            manifest_digest,
        })
    }

    pub fn manifest_id(&self) -> &str {
        &self.draft.manifest_id
    }

    pub const fn manifest_schema_version(&self) -> u16 {
        self.draft.manifest_schema_version
    }

    pub const fn prepared_at(&self) -> i64 {
        self.draft.prepared_at
    }

    pub fn source(&self) -> &RepairSource {
        &self.draft.source
    }

    pub fn target(&self) -> &RepairTarget {
        &self.draft.target
    }

    pub fn expected_state(&self) -> &RepairExpectedState {
        &self.draft.expected_state
    }

    pub const fn writer(&self) -> RepairWriter {
        self.draft.writer
    }

    pub fn mutation(&self) -> &RepairMutation {
        &self.draft.mutation
    }

    pub fn allowed_effects(&self) -> &RepairAllowedEffects {
        &self.draft.allowed_effects
    }

    pub fn rollback(&self) -> &RepairRollbackArtifact {
        &self.draft.rollback
    }

    pub fn post_assertions(&self) -> &RepairPostAssertions {
        &self.draft.post_assertions
    }

    pub fn manifest_digest(&self) -> &RepairDigest {
        &self.manifest_digest
    }

    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        self.draft.canonical_bytes()
    }
}

impl<'de> Deserialize<'de> for RepairManifest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairManifestWire::deserialize(deserializer)?;
        if !matches!(
            wire.manifest_schema_version,
            2 | 3
                | 4
                | 5
                | PREVIOUS_REPAIR_MANIFEST_SCHEMA_VERSION
                | REPAIR_MANIFEST_SCHEMA_VERSION
        ) {
            return Err(D::Error::custom(
                RepairContractError::UnsupportedManifestSchema,
            ));
        }
        let draft = RepairManifestDraft::try_new_for_schema(
            wire.manifest_schema_version,
            wire.manifest_id,
            wire.prepared_at,
            wire.source,
            wire.target,
            wire.expected_state,
            wire.writer,
            wire.mutation,
            wire.allowed_effects,
            wire.rollback,
            wire.post_assertions,
        )
        .map_err(D::Error::custom)?;
        Self::try_new(draft, wire.manifest_digest).map_err(D::Error::custom)
    }
}

fn applicable_deep_complete(report: &LintReport) -> bool {
    report
        .checks()
        .iter()
        .find(|check| check.check_id() == REPAIR_CLASSIFICATION_CHECK_ID)
        .is_some_and(|check| matches!(check.outcome(), LintOutcome::Pass | LintOutcome::Finding))
}

fn applicable_deep_relation_complete(report: &LintReport) -> bool {
    report
        .checks()
        .iter()
        .find(|check| check.check_id() == REPAIR_RELATION_CHECK_ID)
        .is_some_and(|check| matches!(check.outcome(), LintOutcome::Pass | LintOutcome::Finding))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepairChoice {
    ReclassifyMemory {
        selected_finding: LintSemanticFinding,
        after_memory_type: MemoryType,
    },
    RenamePageTitle {
        review_id: String,
        page_id: String,
        before_title: String,
        after_title: String,
    },
    CompleteEntityExtraction {
        review_id: String,
        memory_id: String,
        entity_ids: Vec<String>,
    },
    EntityRelation {
        selection: EntityRelationRepairSelection,
        selected_finding: LintSemanticFinding,
    },
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum RepairChoiceWire {
    ReclassifyMemory {
        selected_finding: LintSemanticFinding,
        after_memory_type: MemoryType,
    },
    RenamePageTitle {
        review_id: String,
        page_id: String,
        before_title: String,
        after_title: String,
    },
    CompleteEntityExtraction {
        review_id: String,
        memory_id: String,
        entity_ids: Vec<String>,
    },
    EntityRelation {
        selection: EntityRelationRepairSelection,
        selected_finding: LintSemanticFinding,
    },
}

impl RepairChoice {
    pub fn reclassify_memory(
        selected_finding: LintSemanticFinding,
        after_memory_type: MemoryType,
    ) -> Result<Self, RepairContractError> {
        if selected_finding.proposed_action() != LintSemanticAction::ReclassifyMemory
            || selected_finding.unresolved_disagreement()
        {
            return Err(RepairContractError::InvalidPrepareRequest);
        }
        Ok(Self::ReclassifyMemory {
            selected_finding,
            after_memory_type,
        })
    }

    pub fn rename_page_title(
        review_id: String,
        page_id: String,
        before_title: String,
        after_title: String,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&review_id)
            || !valid_nonempty(&page_id)
            || !valid_nonempty(&before_title)
            || !valid_nonempty(&after_title)
            || before_title == after_title
        {
            return Err(RepairContractError::InvalidPrepareRequest);
        }
        Ok(Self::RenamePageTitle {
            review_id,
            page_id,
            before_title,
            after_title,
        })
    }

    pub fn complete_entity_extraction(
        review_id: String,
        memory_id: String,
        entity_ids: Vec<String>,
    ) -> Result<Self, RepairContractError> {
        if !valid_nonempty(&review_id) || !valid_nonempty(&memory_id) {
            return Err(RepairContractError::InvalidPrepareRequest);
        }
        RepairMutation::complete_entity_extraction(entity_ids.clone())
            .map_err(|_| RepairContractError::InvalidPrepareRequest)?;
        Ok(Self::CompleteEntityExtraction {
            review_id,
            memory_id,
            entity_ids,
        })
    }

    pub fn entity_relation(
        selection: EntityRelationRepairSelection,
        selected_finding: LintSemanticFinding,
    ) -> Result<Self, RepairContractError> {
        selection
            .validate()
            .map_err(|_| RepairContractError::InvalidPrepareRequest)?;
        let expected_action = match &selection.choice {
            EntityRelationRepairChoice::Add { .. } => LintSemanticAction::AddEntityRelation,
            EntityRelationRepairChoice::Retire { .. } => LintSemanticAction::RemoveEntityRelation,
        };
        if selected_finding.proposed_action() != expected_action
            || selected_finding.unresolved_disagreement()
        {
            return Err(RepairContractError::InvalidPrepareRequest);
        }
        Ok(Self::EntityRelation {
            selection,
            selected_finding,
        })
    }

    pub fn selected_finding(&self) -> Option<&LintSemanticFinding> {
        match self {
            Self::ReclassifyMemory {
                selected_finding, ..
            }
            | Self::EntityRelation {
                selected_finding, ..
            } => Some(selected_finding),
            Self::RenamePageTitle { .. } | Self::CompleteEntityExtraction { .. } => None,
        }
    }

    pub fn after_memory_type(&self) -> Option<&MemoryType> {
        match self {
            Self::ReclassifyMemory {
                after_memory_type, ..
            } => Some(after_memory_type),
            Self::RenamePageTitle { .. }
            | Self::CompleteEntityExtraction { .. }
            | Self::EntityRelation { .. } => None,
        }
    }
}

impl<'de> Deserialize<'de> for RepairChoice {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        match RepairChoiceWire::deserialize(deserializer)? {
            RepairChoiceWire::ReclassifyMemory {
                selected_finding,
                after_memory_type,
            } => Self::reclassify_memory(selected_finding, after_memory_type),
            RepairChoiceWire::RenamePageTitle {
                review_id,
                page_id,
                before_title,
                after_title,
            } => Self::rename_page_title(review_id, page_id, before_title, after_title),
            RepairChoiceWire::CompleteEntityExtraction {
                review_id,
                memory_id,
                entity_ids,
            } => Self::complete_entity_extraction(review_id, memory_id, entity_ids),
            RepairChoiceWire::EntityRelation {
                selection,
                selected_finding,
            } => Self::entity_relation(selection, selected_finding),
        }
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PrepareRepairRequest {
    lint_scope: RepairLintScope,
    general_report: LintReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    deep_report: Option<LintReport>,
    choice: RepairChoice,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareRepairRequestWire {
    lint_scope: RepairLintScope,
    general_report: LintReport,
    #[serde(default)]
    deep_report: Option<LintReport>,
    #[serde(default, deserialize_with = "deserialize_present_optional")]
    choice: PresentOptional<RepairChoice>,
    #[serde(default, deserialize_with = "deserialize_present_optional")]
    selected_finding: PresentOptional<LintSemanticFinding>,
    #[serde(default, deserialize_with = "deserialize_present_optional")]
    after_memory_type: PresentOptional<MemoryType>,
}

struct PresentOptional<T> {
    present: bool,
    value: Option<T>,
}

impl<T> Default for PresentOptional<T> {
    fn default() -> Self {
        Self {
            present: false,
            value: None,
        }
    }
}

fn deserialize_present_optional<'de, D, T>(deserializer: D) -> Result<PresentOptional<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Ok(PresentOptional {
        present: true,
        value: Option::<T>::deserialize(deserializer)?,
    })
}

impl PrepareRepairRequest {
    pub fn try_new(
        lint_scope: RepairLintScope,
        general_report: LintReport,
        deep_report: LintReport,
        selected_finding: LintSemanticFinding,
        after_memory_type: MemoryType,
    ) -> Result<Self, RepairContractError> {
        let choice = RepairChoice::reclassify_memory(selected_finding, after_memory_type)?;
        Self::try_new_with_choice(lint_scope, general_report, Some(deep_report), choice)
    }

    pub fn try_new_with_choice(
        lint_scope: RepairLintScope,
        general_report: LintReport,
        deep_report: Option<LintReport>,
        choice: RepairChoice,
    ) -> Result<Self, RepairContractError> {
        let general_valid = lint_scope.matches_report_scope_kind(general_report.scope())
            && general_report.profile() == LintProfile::General
            && general_report.complete();
        let deep_valid = deep_report.as_ref().is_none_or(|deep| {
            lint_scope.matches_report_scope_kind(deep.scope())
                && deep.profile() == LintProfile::Deep
                && general_report.scope() == deep.scope()
        });
        let choice_valid = match &choice {
            RepairChoice::ReclassifyMemory { .. } => deep_report
                .as_ref()
                .is_some_and(|deep| applicable_deep_complete(deep) && deep.agent_work().is_some()),
            RepairChoice::EntityRelation { .. } => deep_report.as_ref().is_some_and(|deep| {
                applicable_deep_relation_complete(deep) && deep.agent_work().is_some()
            }),
            RepairChoice::RenamePageTitle { .. }
            | RepairChoice::CompleteEntityExtraction { .. } => true,
        };
        if !general_valid || !deep_valid || !choice_valid {
            return Err(RepairContractError::InvalidPrepareRequest);
        }
        Ok(Self {
            lint_scope,
            general_report,
            deep_report,
            choice,
        })
    }

    pub fn lint_scope(&self) -> &RepairLintScope {
        &self.lint_scope
    }

    pub fn general_report(&self) -> &LintReport {
        &self.general_report
    }

    pub fn deep_report(&self) -> Option<&LintReport> {
        self.deep_report.as_ref()
    }

    pub fn choice(&self) -> &RepairChoice {
        &self.choice
    }

    pub fn selected_finding(&self) -> Option<&LintSemanticFinding> {
        self.choice.selected_finding()
    }

    pub fn after_memory_type(&self) -> Option<&MemoryType> {
        self.choice.after_memory_type()
    }
}

impl<'de> Deserialize<'de> for PrepareRepairRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PrepareRepairRequestWire::deserialize(deserializer)?;
        let choice = match (
            wire.choice.present,
            wire.choice.value,
            wire.selected_finding.present,
            wire.selected_finding.value,
            wire.after_memory_type.present,
            wire.after_memory_type.value,
        ) {
            (true, Some(choice), false, None, false, None) => choice,
            (false, None, true, Some(selected_finding), true, Some(after_memory_type)) => {
                RepairChoice::reclassify_memory(selected_finding, after_memory_type)
                    .map_err(D::Error::custom)?
            }
            _ => return Err(D::Error::custom(RepairContractError::InvalidPrepareRequest)),
        };
        Self::try_new_with_choice(
            wire.lint_scope,
            wire.general_report,
            wire.deep_report,
            choice,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
/// Intent-binding request for cooperating agent workflows. The exact phrase is
/// deliberately not a local authentication or malicious-process boundary.
pub struct ApplyRepairRequest {
    manifest_id: String,
    approved_manifest_digest: RepairDigest,
    approval: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplyRepairRequestWire {
    manifest_id: String,
    approved_manifest_digest: RepairDigest,
    approval: String,
}

impl ApplyRepairRequest {
    pub fn try_new(
        manifest_id: String,
        approved_manifest_digest: RepairDigest,
        approval: String,
    ) -> Result<Self, RepairContractError> {
        let expected = format!(
            "apply repair {} {}",
            manifest_id,
            approved_manifest_digest.as_str()
        );
        if !valid_manifest_id(&manifest_id) || approval != expected {
            return Err(RepairContractError::InvalidApplyRequest);
        }
        Ok(Self {
            manifest_id,
            approved_manifest_digest,
            approval,
        })
    }

    pub fn manifest_id(&self) -> &str {
        &self.manifest_id
    }

    pub fn approved_manifest_digest(&self) -> &RepairDigest {
        &self.approved_manifest_digest
    }

    pub fn approval(&self) -> &str {
        &self.approval
    }
}

impl<'de> Deserialize<'de> for ApplyRepairRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ApplyRepairRequestWire::deserialize(deserializer)?;
        Self::try_new(
            wire.manifest_id,
            wire.approved_manifest_digest,
            wire.approval,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairApplyReceiptDraft {
    receipt_schema_version: u16,
    manifest_id: String,
    manifest_digest: RepairDigest,
    applied_at: i64,
    before_target_receipt: RepairDigest,
    after_target_receipt: RepairDigest,
    non_target_before: RepairDigest,
    non_target_after: RepairDigest,
    #[serde(skip_serializing_if = "Option::is_none")]
    post_apply_db_digest: Option<RepairDigest>,
    actual_effects: RepairAllowedEffects,
    writer: RepairWriter,
}

impl RepairApplyReceiptDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        manifest_id: String,
        manifest_digest: RepairDigest,
        applied_at: i64,
        before_target_receipt: RepairDigest,
        after_target_receipt: RepairDigest,
        non_target_before: RepairDigest,
        non_target_after: RepairDigest,
        post_apply_db_digest: RepairDigest,
        actual_effects: RepairAllowedEffects,
        writer: RepairWriter,
    ) -> Result<Self, RepairContractError> {
        let receipt_schema_version = match writer {
            RepairWriter::EntityRelation => REPAIR_RELATION_RECEIPT_SCHEMA_VERSION,
            RepairWriter::RenamePageTitle | RepairWriter::CompleteEntityExtraction => {
                REPAIR_RECEIPT_SCHEMA_VERSION
            }
            _ => PREVIOUS_REPAIR_RECEIPT_SCHEMA_VERSION,
        };
        Self::try_new_for_schema(
            receipt_schema_version,
            manifest_id,
            manifest_digest,
            applied_at,
            before_target_receipt,
            after_target_receipt,
            non_target_before,
            non_target_after,
            post_apply_db_digest,
            actual_effects,
            writer,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn try_new_for_schema(
        receipt_schema_version: u16,
        manifest_id: String,
        manifest_digest: RepairDigest,
        applied_at: i64,
        before_target_receipt: RepairDigest,
        after_target_receipt: RepairDigest,
        non_target_before: RepairDigest,
        non_target_after: RepairDigest,
        post_apply_db_digest: RepairDigest,
        actual_effects: RepairAllowedEffects,
        writer: RepairWriter,
    ) -> Result<Self, RepairContractError> {
        if !matches!(
            receipt_schema_version,
            2 | 3 | 4 | REPAIR_RECEIPT_SCHEMA_VERSION | REPAIR_RELATION_RECEIPT_SCHEMA_VERSION
        ) {
            return Err(RepairContractError::InvalidReceipt);
        }
        let aggregate_writer = matches!(
            writer,
            RepairWriter::RenamePageTitle | RepairWriter::CompleteEntityExtraction
        );
        let relation_writer = writer == RepairWriter::EntityRelation;
        if !valid_manifest_id(&manifest_id)
            || applied_at <= 0
            || before_target_receipt == after_target_receipt
            || non_target_before != non_target_after
            || !repair_receipt_effects_match(writer, &actual_effects)
            || receipt_schema_version
                < match writer {
                    RepairWriter::RenamePageTitle | RepairWriter::CompleteEntityExtraction => 5,
                    RepairWriter::UnstageOrphanRevision
                    | RepairWriter::DeleteMemoryEntityLink
                    | RepairWriter::ArchiveEmptySourcePage
                    | RepairWriter::QuarantineStalePageProjection => 4,
                    _ => 2,
                }
            || (relation_writer && receipt_schema_version != REPAIR_RELATION_RECEIPT_SCHEMA_VERSION)
            || (aggregate_writer && receipt_schema_version != REPAIR_RECEIPT_SCHEMA_VERSION)
            || (!aggregate_writer
                && !relation_writer
                && receipt_schema_version > PREVIOUS_REPAIR_RECEIPT_SCHEMA_VERSION)
        {
            return Err(RepairContractError::InvalidReceipt);
        }
        Ok(Self {
            receipt_schema_version,
            manifest_id,
            manifest_digest,
            applied_at,
            before_target_receipt,
            after_target_receipt,
            non_target_before,
            non_target_after,
            post_apply_db_digest: Some(post_apply_db_digest),
            actual_effects,
            writer,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn try_new_legacy_v1(
        manifest_id: String,
        manifest_digest: RepairDigest,
        applied_at: i64,
        before_target_receipt: RepairDigest,
        after_target_receipt: RepairDigest,
        non_target_before: RepairDigest,
        non_target_after: RepairDigest,
        actual_effects: RepairAllowedEffects,
        writer: RepairWriter,
    ) -> Result<Self, RepairContractError> {
        if !valid_manifest_id(&manifest_id)
            || applied_at <= 0
            || before_target_receipt == after_target_receipt
            || non_target_before != non_target_after
            || writer != RepairWriter::ReclassifyMemory
        {
            return Err(RepairContractError::InvalidReceipt);
        }
        Ok(Self {
            receipt_schema_version: 1,
            manifest_id,
            manifest_digest,
            applied_at,
            before_target_receipt,
            after_target_receipt,
            non_target_before,
            non_target_after,
            post_apply_db_digest: None,
            actual_effects,
            writer,
        })
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

fn repair_receipt_effects_match(
    writer: RepairWriter,
    actual_effects: &RepairAllowedEffects,
) -> bool {
    matches!(
        (actual_effects.owner(), writer, actual_effects.fields()),
        (
            RepairTarget::EntityRelation { .. },
            RepairWriter::EntityRelation,
            [
                RepairMemoryField::RelationEdges,
                RepairMemoryField::CommunityGraphState,
                RepairMemoryField::RelationVocabulary,
                RepairMemoryField::RelationActivity,
                RepairMemoryField::RelationReviewQueue,
            ]
        ) | (
            RepairTarget::Memory { .. },
            RepairWriter::ReclassifyMemory,
            [RepairMemoryField::MemoryType]
        ) | (
            RepairTarget::Memory { .. },
            RepairWriter::NormalizeMemorySourceAgent,
            [RepairMemoryField::SourceAgent]
        ) | (
            RepairTarget::Memory { .. },
            RepairWriter::ClearMemorySupersedes,
            [RepairMemoryField::Supersedes]
        ) | (
            RepairTarget::Memory { .. },
            RepairWriter::UnstageOrphanRevision,
            [RepairMemoryField::PendingRevision]
        ) | (
            RepairTarget::Tag { .. },
            RepairWriter::DeleteTagRow,
            [RepairMemoryField::TagRow]
        ) | (
            RepairTarget::MemoryEntityLink { .. },
            RepairWriter::DeleteMemoryEntityLink,
            [RepairMemoryField::MemoryEntityLink]
        ) | (
            RepairTarget::PageLink { .. },
            RepairWriter::BindPageLink,
            [RepairMemoryField::TargetPageId]
        ) | (
            RepairTarget::PageProjection { .. },
            RepairWriter::RegeneratePageProjection,
            [RepairMemoryField::PageProjection]
        ) | (
            RepairTarget::PageProjection { .. },
            RepairWriter::QuarantineStalePageProjection,
            [RepairMemoryField::PageProjectionQuarantine]
        ) | (
            RepairTarget::Page { .. },
            RepairWriter::ArchiveEmptySourcePage,
            [RepairMemoryField::PageStatus]
        ) | (
            RepairTarget::PageProjection { .. },
            RepairWriter::RenamePageTitle,
            [
                RepairMemoryField::PageTitle,
                RepairMemoryField::PageVersion,
                RepairMemoryField::PageEmbedding,
                RepairMemoryField::PageProjection,
            ]
        ) | (
            RepairTarget::MemoryEntityExtraction { .. },
            RepairWriter::CompleteEntityExtraction,
            // Two fields is a receipt for a manifest prepared before #708;
            // three is one that also establishes the entity.
            [
                RepairMemoryField::MemoryEntityLinks,
                RepairMemoryField::EnrichmentStep,
            ] | [
                RepairMemoryField::MemoryEntityLinks,
                RepairMemoryField::EnrichmentStep,
                RepairMemoryField::EntityEstablishment,
            ]
        )
    )
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairApplyReceipt {
    #[serde(flatten)]
    draft: RepairApplyReceiptDraft,
    receipt_digest: RepairDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairApplyReceiptWire {
    receipt_schema_version: u16,
    manifest_id: String,
    manifest_digest: RepairDigest,
    applied_at: i64,
    before_target_receipt: RepairDigest,
    after_target_receipt: RepairDigest,
    non_target_before: RepairDigest,
    non_target_after: RepairDigest,
    #[serde(default)]
    post_apply_db_digest: Option<RepairDigest>,
    actual_effects: RepairAllowedEffects,
    writer: RepairWriter,
    receipt_digest: RepairDigest,
}

impl RepairApplyReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        manifest_id: String,
        manifest_digest: RepairDigest,
        applied_at: i64,
        before_target_receipt: RepairDigest,
        after_target_receipt: RepairDigest,
        non_target_before: RepairDigest,
        non_target_after: RepairDigest,
        post_apply_db_digest: RepairDigest,
        actual_effects: RepairAllowedEffects,
        writer: RepairWriter,
        receipt_digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        let draft = RepairApplyReceiptDraft::try_new(
            manifest_id,
            manifest_digest,
            applied_at,
            before_target_receipt,
            after_target_receipt,
            non_target_before,
            non_target_after,
            post_apply_db_digest,
            actual_effects,
            writer,
        )?;
        Ok(Self::from_draft(draft, receipt_digest))
    }

    pub fn from_draft(draft: RepairApplyReceiptDraft, receipt_digest: RepairDigest) -> Self {
        Self {
            draft,
            receipt_digest,
        }
    }

    pub fn manifest_id(&self) -> &str {
        &self.draft.manifest_id
    }

    pub fn manifest_digest(&self) -> &RepairDigest {
        &self.draft.manifest_digest
    }

    pub fn receipt_digest(&self) -> &RepairDigest {
        &self.receipt_digest
    }

    pub fn actual_effects(&self) -> &RepairAllowedEffects {
        &self.draft.actual_effects
    }

    pub fn before_target_receipt(&self) -> &RepairDigest {
        &self.draft.before_target_receipt
    }

    pub fn after_target_receipt(&self) -> &RepairDigest {
        &self.draft.after_target_receipt
    }

    pub fn non_target_before(&self) -> &RepairDigest {
        &self.draft.non_target_before
    }

    pub fn non_target_after(&self) -> &RepairDigest {
        &self.draft.non_target_after
    }

    pub fn post_apply_db_digest(&self) -> Option<&RepairDigest> {
        self.draft.post_apply_db_digest.as_ref()
    }

    pub const fn writer(&self) -> RepairWriter {
        self.draft.writer
    }

    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        self.draft.canonical_bytes()
    }
}

impl<'de> Deserialize<'de> for RepairApplyReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairApplyReceiptWire::deserialize(deserializer)?;
        let draft = match wire.receipt_schema_version {
            1 if wire.post_apply_db_digest.is_none() => RepairApplyReceiptDraft::try_new_legacy_v1(
                wire.manifest_id,
                wire.manifest_digest,
                wire.applied_at,
                wire.before_target_receipt,
                wire.after_target_receipt,
                wire.non_target_before,
                wire.non_target_after,
                wire.actual_effects,
                wire.writer,
            ),
            2 | 3 | 4 | REPAIR_RECEIPT_SCHEMA_VERSION | REPAIR_RELATION_RECEIPT_SCHEMA_VERSION => {
                RepairApplyReceiptDraft::try_new_for_schema(
                    wire.receipt_schema_version,
                    wire.manifest_id,
                    wire.manifest_digest,
                    wire.applied_at,
                    wire.before_target_receipt,
                    wire.after_target_receipt,
                    wire.non_target_before,
                    wire.non_target_after,
                    wire.post_apply_db_digest
                        .ok_or(RepairContractError::InvalidReceipt)
                        .map_err(D::Error::custom)?,
                    wire.actual_effects,
                    wire.writer,
                )
            }
            _ => Err(RepairContractError::InvalidReceipt),
        }
        .map_err(D::Error::custom)?;
        Ok(Self::from_draft(draft, wire.receipt_digest))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyRepairRequest {
    manifest_id: String,
    manifest_digest: RepairDigest,
    apply_receipt_digest: RepairDigest,
    general_report: LintReport,
    #[serde(skip_serializing_if = "Option::is_none")]
    deep_report: Option<LintReport>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_apply: Option<ApplyRepairRequest>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct VerifyRepairRequestWire {
    manifest_id: String,
    manifest_digest: RepairDigest,
    apply_receipt_digest: RepairDigest,
    general_report: LintReport,
    #[serde(default)]
    deep_report: Option<LintReport>,
    #[serde(default)]
    next_apply: Option<ApplyRepairRequest>,
}

impl VerifyRepairRequest {
    pub fn try_new(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        general_report: LintReport,
        deep_report: LintReport,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_deep_backed(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            general_report,
            deep_report,
        )
    }

    pub fn try_new_deep_backed(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        general_report: LintReport,
        deep_report: LintReport,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_with_next_apply(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            general_report,
            deep_report,
            None,
        )
    }

    pub fn try_new_general_only(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        general_report: LintReport,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_general_only_with_next_apply(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            general_report,
            None,
        )
    }

    pub fn try_new_with_next_apply(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        general_report: LintReport,
        deep_report: LintReport,
        next_apply: Option<ApplyRepairRequest>,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_with_optional_deep_and_next_apply(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            general_report,
            Some(deep_report),
            next_apply,
        )
    }

    pub fn try_new_general_only_with_next_apply(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        general_report: LintReport,
        next_apply: Option<ApplyRepairRequest>,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_with_optional_deep_and_next_apply(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            general_report,
            None,
            next_apply,
        )
    }

    pub fn try_new_with_optional_deep_and_next_apply(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        general_report: LintReport,
        deep_report: Option<LintReport>,
        next_apply: Option<ApplyRepairRequest>,
    ) -> Result<Self, RepairContractError> {
        if !valid_manifest_id(&manifest_id)
            || general_report.profile() != LintProfile::General
            || !general_report.complete()
            || deep_report.as_ref().is_some_and(|deep| {
                deep.profile() != LintProfile::Deep
                    || general_report.scope() != deep.scope()
                    || general_report.producer_receipt() != deep.producer_receipt()
            })
            || next_apply
                .as_ref()
                .is_some_and(|next| next.manifest_id() == manifest_id)
        {
            return Err(RepairContractError::InvalidVerifyRequest);
        }
        Ok(Self {
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            general_report,
            deep_report,
            next_apply,
        })
    }

    pub fn manifest_id(&self) -> &str {
        &self.manifest_id
    }

    pub fn manifest_digest(&self) -> &RepairDigest {
        &self.manifest_digest
    }

    pub fn apply_receipt_digest(&self) -> &RepairDigest {
        &self.apply_receipt_digest
    }

    pub fn general_report(&self) -> &LintReport {
        &self.general_report
    }

    pub fn deep_report(&self) -> Option<&LintReport> {
        self.deep_report.as_ref()
    }

    pub fn next_apply(&self) -> Option<&ApplyRepairRequest> {
        self.next_apply.as_ref()
    }
}

impl<'de> Deserialize<'de> for VerifyRepairRequest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = VerifyRepairRequestWire::deserialize(deserializer)?;
        Self::try_new_with_optional_deep_and_next_apply(
            wire.manifest_id,
            wire.manifest_digest,
            wire.apply_receipt_digest,
            wire.general_report,
            wire.deep_report,
            wire.next_apply,
        )
        .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairVerificationReceiptDraft {
    receipt_schema_version: u16,
    manifest_id: String,
    manifest_digest: RepairDigest,
    apply_receipt_digest: RepairDigest,
    verified_at: i64,
    general_snapshots: LintSnapshotReceipts,
    #[serde(skip_serializing_if = "Option::is_none")]
    deep_snapshots: Option<LintSnapshotReceipts>,
}

impl RepairVerificationReceiptDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        verified_at: i64,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: LintSnapshotReceipts,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_for_schema(
            REPAIR_VERIFICATION_RECEIPT_SCHEMA_VERSION,
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            verified_at,
            general_snapshots,
            Some(deep_snapshots),
        )
    }

    pub fn try_new_general_only(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        verified_at: i64,
        general_snapshots: LintSnapshotReceipts,
    ) -> Result<Self, RepairContractError> {
        Self::try_new_for_schema(
            REPAIR_VERIFICATION_RECEIPT_SCHEMA_VERSION,
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            verified_at,
            general_snapshots,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn try_new_for_schema(
        receipt_schema_version: u16,
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        verified_at: i64,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: Option<LintSnapshotReceipts>,
    ) -> Result<Self, RepairContractError> {
        if !matches!(
            receipt_schema_version,
            2 | 3 | REPAIR_VERIFICATION_RECEIPT_SCHEMA_VERSION
        ) || (receipt_schema_version == 2 && deep_snapshots.is_none())
        {
            return Err(RepairContractError::InvalidReceipt);
        }
        if !valid_manifest_id(&manifest_id) || verified_at <= 0 {
            return Err(RepairContractError::InvalidReceipt);
        }
        Ok(Self {
            receipt_schema_version,
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            verified_at,
            general_snapshots,
            deep_snapshots,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn try_new_legacy_v1(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        verified_at: i64,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: LintSnapshotReceipts,
    ) -> Result<Self, RepairContractError> {
        if !valid_manifest_id(&manifest_id) || verified_at <= 0 {
            return Err(RepairContractError::InvalidReceipt);
        }
        Ok(Self {
            receipt_schema_version: 1,
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            verified_at,
            general_snapshots,
            deep_snapshots: Some(deep_snapshots),
        })
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepairVerificationReceipt {
    #[serde(flatten)]
    draft: RepairVerificationReceiptDraft,
    receipt_digest: RepairDigest,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RepairVerificationReceiptWire {
    receipt_schema_version: u16,
    manifest_id: String,
    manifest_digest: RepairDigest,
    apply_receipt_digest: RepairDigest,
    verified_at: i64,
    general_snapshots: LintSnapshotReceipts,
    #[serde(default)]
    deep_snapshots: Option<LintSnapshotReceipts>,
    receipt_digest: RepairDigest,
}

impl RepairVerificationReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn try_new(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        verified_at: i64,
        general_snapshots: LintSnapshotReceipts,
        deep_snapshots: LintSnapshotReceipts,
        receipt_digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        let draft = RepairVerificationReceiptDraft::try_new(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            verified_at,
            general_snapshots,
            deep_snapshots,
        )?;
        Ok(Self::from_draft(draft, receipt_digest))
    }

    #[allow(clippy::too_many_arguments)]
    pub fn try_new_general_only(
        manifest_id: String,
        manifest_digest: RepairDigest,
        apply_receipt_digest: RepairDigest,
        verified_at: i64,
        general_snapshots: LintSnapshotReceipts,
        receipt_digest: RepairDigest,
    ) -> Result<Self, RepairContractError> {
        let draft = RepairVerificationReceiptDraft::try_new_general_only(
            manifest_id,
            manifest_digest,
            apply_receipt_digest,
            verified_at,
            general_snapshots,
        )?;
        Ok(Self::from_draft(draft, receipt_digest))
    }

    pub fn from_draft(draft: RepairVerificationReceiptDraft, receipt_digest: RepairDigest) -> Self {
        Self {
            draft,
            receipt_digest,
        }
    }

    pub fn receipt_digest(&self) -> &RepairDigest {
        &self.receipt_digest
    }

    pub fn manifest_id(&self) -> &str {
        &self.draft.manifest_id
    }

    pub fn manifest_digest(&self) -> &RepairDigest {
        &self.draft.manifest_digest
    }

    pub fn apply_receipt_digest(&self) -> &RepairDigest {
        &self.draft.apply_receipt_digest
    }

    pub fn general_snapshots(&self) -> &LintSnapshotReceipts {
        &self.draft.general_snapshots
    }

    pub fn deep_snapshots(&self) -> Option<&LintSnapshotReceipts> {
        self.draft.deep_snapshots.as_ref()
    }

    pub fn canonical_unsigned_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        self.draft.canonical_bytes()
    }
}

impl<'de> Deserialize<'de> for RepairVerificationReceipt {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = RepairVerificationReceiptWire::deserialize(deserializer)?;
        let draft = match wire.receipt_schema_version {
            1 => RepairVerificationReceiptDraft::try_new_legacy_v1(
                wire.manifest_id,
                wire.manifest_digest,
                wire.apply_receipt_digest,
                wire.verified_at,
                wire.general_snapshots,
                wire.deep_snapshots
                    .ok_or(RepairContractError::InvalidReceipt)
                    .map_err(D::Error::custom)?,
            ),
            2 => RepairVerificationReceiptDraft::try_new_for_schema(
                2,
                wire.manifest_id,
                wire.manifest_digest,
                wire.apply_receipt_digest,
                wire.verified_at,
                wire.general_snapshots,
                wire.deep_snapshots,
            ),
            3 => RepairVerificationReceiptDraft::try_new_for_schema(
                3,
                wire.manifest_id,
                wire.manifest_digest,
                wire.apply_receipt_digest,
                wire.verified_at,
                wire.general_snapshots,
                wire.deep_snapshots,
            ),
            REPAIR_VERIFICATION_RECEIPT_SCHEMA_VERSION => {
                RepairVerificationReceiptDraft::try_new_for_schema(
                    REPAIR_VERIFICATION_RECEIPT_SCHEMA_VERSION,
                    wire.manifest_id,
                    wire.manifest_digest,
                    wire.apply_receipt_digest,
                    wire.verified_at,
                    wire.general_snapshots,
                    wire.deep_snapshots,
                )
            }
            _ => Err(RepairContractError::InvalidReceipt),
        }
        .map_err(D::Error::custom)?;
        Ok(Self::from_draft(draft, wire.receipt_digest))
    }
}
