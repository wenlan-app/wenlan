// SPDX-License-Identifier: AGPL-3.0-only
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../i18n";
import * as api from "../../lib/tauri";
import type { Entity, MemoryItem, Page, RefinementPayload } from "../../lib/tauri";
import {
  repairProgressKey,
  sha256Hex,
  writeRepairProgress,
  type RepairProgressRecord,
} from "../../lib/repairWorkflow";
import type { RepairApplyReceipt, RepairLintReport, RepairManifest, RepairVerificationReceipt, RepairPrepareOperationRequest } from "../../lib/repairTypes";
import SourceRepairReview from "./SourceRepairReview";
import { writeRepairPreparation, type RepairPreparationRecord } from "../../lib/repairPreparation";
import type { ReviewItem } from "./useReviewQueue";

type RepairItem = Extract<ReviewItem, { kind: "refinement" }>;
type RepairPayload = Extract<RefinementPayload, { action: "lint_repair_review" }>;

vi.mock("../../lib/tauri", async (original) => ({
  ...(await original<typeof import("../../lib/tauri")>()),
  getMemoryDetail: vi.fn(),
  getEntityDetail: vi.fn(),
  getPage: vi.fn(),
  listEntities: vi.fn(),
  repairLint: vi.fn(),
  repairPrepareOperation: vi.fn(),
  repairPrepareOperationStatus: vi.fn(),
  repairPrepareOperationCancel: vi.fn(),
  repairOperationStatus: vi.fn(),
  repairCancel: vi.fn(),
  repairRecovery: vi.fn(),
  repairApply: vi.fn(),
  repairVerify: vi.fn(),
  repairResumeRuntime: vi.fn(),
  repairValidateManifest: vi.fn(),
  getActivity: vi.fn(),
}));

const memory: MemoryItem = {
  source_id: "memory-1",
  title: "A source-backed memory",
  content: "The original memory text.",
  summary: null,
  memory_type: "fact",
  domain: null,
  space: null,
  source_agent: "test",
  confidence: 0.9,
  confirmed: true,
  pinned: false,
  supersedes: null,
  last_modified: 1,
  chunk_count: 1,
};

const page: Page = {
  id: "page-1",
  title: "Current page title",
  summary: "Page summary",
  content: "Current page content.",
  entity_id: null,
  domain: null,
  space: null,
  source_memory_ids: [memory.source_id],
  version: 2,
  status: "active",
  created_at: "2026-01-01T00:00:00Z",
  last_compiled: "2026-01-01T00:00:00Z",
  last_modified: "2026-01-01T00:00:00Z",
};

const entities: Entity[] = [
  { id: "entity-1", name: "Same scope", entity_type: "concept", domain: null, space: "work", source_agent: null, confidence: null, confirmed: true, created_at: 1, updated_at: 1, memory_count: 1, status: "established", established_by: "manual" },
  { id: "entity-2", name: "Other scope", entity_type: "concept", domain: null, space: "other", source_agent: null, confidence: null, confirmed: true, created_at: 1, updated_at: 1, memory_count: 1, status: "established", established_by: "manual" },
];

const basePayload = (checkId: string): RefinementPayload => ({
  action: "lint_repair_review",
  check_id: checkId,
  occurrence_digest: "a".repeat(64),
  owner_binding_digest: "b".repeat(64),
  issue: "The current source needs a checked repair.",
  choices: [],
  suggested_research_queries: [],
});

function itemFor(checkId: string, sourceId = memory.source_id): RepairItem {
  return {
    kind: "refinement",
    id: `review-${checkId}`,
    action: "lint_repair_review",
    sourceIds: [sourceId],
    payload: basePayload(checkId),
    confidence: 0.8,
    timestampMs: 1,
  };
}

function lintReport(profile: "general" | "deep", checkId: string, finding = false, complete = true): RepairLintReport {
  return {
    report_schema_version: 1,
    check_catalog_version: 1,
    profile,
    scope: { kind: "uncategorized" },
    capability_context: "daemon_operator_endpoint_unauthenticated_unverified",
    snapshots: { db: { mode: "transactional_read_only", analysis_digest: "a".repeat(16), post_run_digest: null }, pages: { mode: "best_effort", before_scan_digest: "b".repeat(16), after_scan_digest: null } },
    config_fingerprint: "c".repeat(16),
    producer_receipt: { runtime_commit: null },
    agent_work: profile === "deep" ? { work_schema_version: 1, work_digest: "d".repeat(16), populations: [], records: [], candidates: [] } : null,
    checks: [{
      check_id: checkId,
      outcome: finding ? "finding" : "pass",
      gate_effect: "actionable",
      severity: finding ? "warning" : "info",
      applicability: "applicable",
      precondition: "ready",
      coverage: { method: "exact_aggregate", authorized_denominator: 1, evaluated: 1, evidence_cap: 100, truncated: false, evidence_returned: finding ? 1 : 0 }, metrics: [], summary_code: finding ? "finding_detected" : "check_passed",
      evidence: finding ? [{
        kind: "semantic_finding",
        finding: {
          candidate_id: 1,
          proposed_action: "reclassify_memory",
          reason_code: "classification_mismatch",
          confidence_basis_points: 9000,
          provider_route: "on_device",
          evidence_ids: ["abfebda9ecdb7fc0"],
          counterevidence_ids: [],
          unresolved_disagreement: false,
        },
      }] : [],
      duration_ms: 1,
    }],
    totals: { checks: 1, passed: finding ? 0 : 1, findings: finding ? 1 : 0, actionable_findings: finding ? 1 : 0, advisory_findings: 0, incomplete: 0 },
    complete,
  } as RepairLintReport;
}

async function manifestFor(item: RepairItem): Promise<RepairManifest> {
  const payload = item.payload as Extract<RefinementPayload, { action: "lint_repair_review" }>;
  const isPage = payload.check_id === "pages.duplicate_active_titles";
  const isExtraction = payload.check_id === "memories.enrichment_failures";
  const unsigned = {
    manifest_schema_version: 1,
    manifest_id: `manifest-${item.id}`,
    prepared_at: 123,
    source: {
      report_schema_version: 1,
      check_catalog_version: 1,
      lint_scope: { kind: "uncategorized" as const },
      report_scope: { kind: "uncategorized" as const },
      check_id: payload.check_id,
      finding: null,
      deterministic_evidence: [],
      general_snapshots: {},
      deep_snapshots: null,
      general_producer_receipt: { runtime_commit: null },
      deep_producer_receipt: null,
      agent_work_digest: null,
      review_binding: { review_id: item.id, occurrence_digest: payload.occurrence_digest, owner_ids: [...item.sourceIds] },
    },
    target: isPage
      ? { kind: "page" as const, page_id: item.sourceIds[0], scope: { kind: "uncategorized" as const } }
      : isExtraction
        ? { kind: "memory_entity_extraction" as const, memory_id: item.sourceIds[0], step: "entity_extract" as const, entity_ids: ["entity-1"], scope: { kind: "uncategorized" as const } }
        : { kind: "memory" as const, source_id: item.sourceIds[0], scope: { kind: "uncategorized" as const } },
    expected_state: { version: null, canonical_receipt: "e".repeat(64) },
    writer: isPage ? "rename_page_title" : isExtraction ? "complete_entity_extraction" : "reclassify_memory",
    mutation: isPage
      ? { kind: "rename_page_title" as const, before_title: page.title, after_title: "New title", after_embedding_hex: "00" }
      : isExtraction
        ? { kind: "complete_entity_extraction" as const, entity_ids: ["entity-1"] }
        : { kind: "reclassify_memory" as const, before_memory_type: "fact" as const, after_memory_type: "preference" as const },
    allowed_effects: {
      owner: isPage
        ? { kind: "page" as const, page_id: item.sourceIds[0], scope: { kind: "uncategorized" as const } }
        : { kind: "memory" as const, source_id: item.sourceIds[0], scope: { kind: "uncategorized" as const } },
      fields: [isPage ? "page_title" as const : isExtraction ? "memory_entity_links" as const : "memory_type" as const],
    },
    rollback: { format_version: 1, relative_path: "rollback.json", digest: "f".repeat(64) },
    post_assertions: {
      target_check_id: payload.check_id,
      target_evidence_id: "1".repeat(64),
      general_baseline: [],
      deep_baseline: [],
      target_record_set: null,
      verification_policy: { kind: isPage || isExtraction ? "general_only" as const : "applicable_checks" as const, ...(isPage || isExtraction ? {} : { required_deep_check_ids: ["memories.semantic.classification"] }) },
      require_complete_general: true,
      reject_new_actionable: true,
      reject_new_incomplete: true,
      allowed_non_target_check_deltas: [],
    },
  };
  return { ...unsigned, manifest_digest: await sha256Hex(JSON.stringify(unsigned)) } as RepairManifest;
}

function applyReceiptFor(manifest: RepairManifest): RepairApplyReceipt {
  return {
    receipt_schema_version: 1,
    manifest_id: manifest.manifest_id,
    manifest_digest: manifest.manifest_digest,
    applied_at: 456,
    before_target_receipt: "1".repeat(64),
    after_target_receipt: "2".repeat(64),
    non_target_before: "3".repeat(64),
    non_target_after: "4".repeat(64),
    actual_effects: manifest.allowed_effects,
    writer: manifest.writer,
    receipt_digest: "5".repeat(64),
  };
}

function verificationFor(manifest: RepairManifest, receipt: RepairApplyReceipt): RepairVerificationReceipt {
  return {
    receipt_schema_version: 1,
    manifest_id: manifest.manifest_id,
    manifest_digest: manifest.manifest_digest,
    apply_receipt_digest: receipt.receipt_digest,
    verified_at: 789,
    general_snapshots: {},
    deep_snapshots: null,
    receipt_digest: "6".repeat(64),
  };
}

function activityResponse(): api.ActivityResponse {
  return {
    state: "up_to_date",
    last_activity_at: null,
    assets: [],
    everyday: { job: "everyday", lane: "on_device", model: "test", mode: "pinned", available: true },
    synthesis: { job: "synthesis", lane: "on_device", model: "test", mode: "pinned", available: true },
    refinement: { ready_for_review: 0, not_ready: 0, groups: [] },
  };
}

function mount(item: RepairItem, onVerified = vi.fn(), onBusyChange = vi.fn()) {
  render(<SourceRepairReview item={item} onBusyChange={onBusyChange} onVerified={onVerified} />);
  return { user: userEvent.setup(), onVerified, onBusyChange };
}

function mockPrepared(manifest: RepairManifest) {
  vi.mocked(api.repairPrepareOperation).mockImplementation(async (request: RepairPrepareOperationRequest) => ({
    operation_id: request.operation_id,
    state: { phase: "ready", manifest, operation: { manifest_id: manifest.manifest_id, manifest_digest: manifest.manifest_digest, state: { phase: "prepared" } } },
  }));
}

beforeEach(async () => {
  vi.restoreAllMocks();
  localStorage.clear();
  vi.clearAllMocks();
  vi.mocked(api.repairPrepareOperation).mockReset();
  vi.mocked(api.repairPrepareOperationStatus).mockReset();
  vi.mocked(api.repairPrepareOperationCancel).mockReset();
  vi.mocked(api.repairOperationStatus).mockReset().mockImplementation(async (request) => ({ manifest_id: request.manifest_id, manifest_digest: request.approved_manifest_digest, state: { phase: "prepared" } }));
  vi.mocked(api.repairCancel).mockReset().mockImplementation(async (request) => ({ manifest_id: request.manifest_id, manifest_digest: request.approved_manifest_digest, state: { phase: "cancelled", cancelled_at: 123 } }));
  await i18n.changeLanguage("en");
  vi.mocked(api.getMemoryDetail).mockResolvedValue(memory);
  vi.mocked(api.getPage).mockResolvedValue(page);
  vi.mocked(api.listEntities).mockResolvedValue([]);
  vi.mocked(api.repairRecovery).mockResolvedValue(null);
  vi.mocked(api.repairResumeRuntime).mockReset().mockResolvedValue(undefined);
  vi.mocked(api.getActivity).mockResolvedValue(activityResponse());
  (api.repairValidateManifest as unknown as { mockResolvedValue: (value: boolean) => void }).mockResolvedValue(true);
  vi.mocked(api.repairLint).mockImplementation(async (query) => lintReport(query.profile, query.profile === "deep" ? "memories.semantic.classification" : "pages.duplicate_active_titles"));
});

async function savedVerifiedRepair() {
  const item = itemFor("pages.duplicate_active_titles", page.id);
  const manifest = await manifestFor(item);
  const receipt = applyReceiptFor(manifest);
  const verification = verificationFor(manifest, receipt);
  writeRepairProgress({ version: 1, reviewId: item.id,
    checkId: (item.payload as RepairPayload).check_id,
    occurrenceDigest: (item.payload as RepairPayload).occurrence_digest,
    ownerIds: item.sourceIds, phase: "verified", manifest,
    applyReceipt: receipt, verificationReceipt: verification });
  return { item, manifest, verification };
}

describe("SourceRepairReview", () => {
  it("previews a relation with all owners and verifies its own deep check before continuing", async () => {
    const checkId = "kg.semantic.entity_relations";
    const item = { ...itemFor(checkId), sourceIds: ["entity-1", "entity-2", memory.source_id] };
    vi.mocked(api.getEntityDetail).mockImplementation(async (id) => {
      const entity = entities.find((entry) => entry.id === id);
      if (!entity) throw new Error("entity not found");
      return { entity, observations: [], relations: [] };
    });
    vi.mocked(api.getMemoryDetail).mockImplementation(async (id) => id === memory.source_id ? memory : null);
    const base = await manifestFor(item);
    const target = { kind: "entity_relation" as const, relation_id: "edge-new", from_entity: "entity-1", to_entity: "entity-2", review_owner_ids: item.sourceIds, scope: { kind: "global" as const } };
    const manifest: RepairManifest = {
      ...base, manifest_schema_version: 7, target, writer: "entity_relation",
      source: { ...base.source, lint_scope: { kind: "global" }, report_scope: { kind: "global" } },
      mutation: { kind: "entity_relation", change: { kind: "add", requested_relation_type: "works_on", canonical_relation_type: "works_on", source_memory_id: memory.source_id, confidence_basis_points: 8000, retire_relation_ids: [], vocabulary_promotion: null } },
      allowed_effects: { owner: target, fields: ["relation_edges", "community_graph_state", "relation_vocabulary", "relation_activity", "relation_review_queue"] },
      post_assertions: { ...base.post_assertions, verification_policy: { kind: "applicable_checks", required_deep_check_ids: [checkId] } },
    };
    mockPrepared(manifest);
    const applied = { ...applyReceiptFor(manifest), receipt_schema_version: 6 };
    vi.mocked(api.repairApply).mockResolvedValue(applied);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, applied));
    vi.mocked(api.repairLint).mockImplementation(async (query) => lintReport(query.profile, checkId));
    const { user, onVerified } = mount(item);
    await user.selectOptions(await screen.findByLabelText("From entity"), "entity-1");
    await user.selectOptions(screen.getByLabelText("To entity"), "entity-2");
    await user.type(screen.getByLabelText("Relationship name", { exact: false }), "works_on");
    await user.selectOptions(screen.getByLabelText("Supporting memory"), memory.source_id);
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await screen.findByText("Add Same scope → Other scope · works_on.");
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(vi.mocked(api.repairPrepareOperation).mock.calls[0][0].request).toEqual({
      lint_scope: { kind: "global" }, choice: { kind: "entity_relation", selection: { review_id: item.id, choice: {
        kind: "add", from_entity: "entity-1", to_entity: "entity-2", relation_type: "works_on", source_memory_id: memory.source_id,
      } } },
    });
    await user.click(screen.getByRole("button", { name: i18n.t("sourceRepair.applyChange") }));
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    expect(api.repairLint).toHaveBeenCalledWith(expect.objectContaining({ profile: "deep" }));
    expect(vi.mocked(api.repairVerify).mock.calls[0][0].deep_report?.checks[0].check_id).toBe(checkId);
    expect(onVerified).toHaveBeenCalledTimes(1);
  });

  it("waits for native resumption and Activity before Continue on remount", async () => {
    const { item, manifest, verification } = await savedVerifiedRepair();
    let resumed!: () => void;
    let activityReady!: (value: api.ActivityResponse) => void;
    vi.mocked(api.repairResumeRuntime).mockReturnValue(new Promise<void>((resolve) => { resumed = resolve; }));
    vi.mocked(api.getActivity).mockReturnValue(new Promise((resolve) => { activityReady = resolve; }));
    const { user, onVerified } = mount(item);
    await waitFor(() => expect(api.repairResumeRuntime).toHaveBeenCalledTimes(1));
    expect(api.repairResumeRuntime).toHaveBeenCalledWith({ apply: {
      manifest_id: manifest.manifest_id, approved_manifest_digest: manifest.manifest_digest,
      approval: `apply repair ${manifest.manifest_id} ${manifest.manifest_digest}`,
    }, verification_receipt_digest: verification.receipt_digest });
    expect(screen.getByRole("status")).toHaveTextContent(i18n.t("sourceRepair.restoringService"));
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
    expect(api.getActivity).not.toHaveBeenCalled();
    await act(async () => resumed());
    await waitFor(() => expect(api.getActivity).toHaveBeenCalledTimes(1));
    expect(screen.queryByRole("button", { name: "Continue" })).toBeNull();
    expect(onVerified).not.toHaveBeenCalled();
    await act(async () => activityReady(activityResponse()));
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    expect(onVerified).toHaveBeenCalledTimes(1);
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(api.repairVerify).not.toHaveBeenCalled();
  });

  it("retains the verified receipt and retries only resumption after a native failure", async () => {
    const { item } = await savedVerifiedRepair();
    const saved = localStorage.getItem(repairProgressKey(item.id));
    vi.mocked(api.repairResumeRuntime).mockRejectedValueOnce(new Error("repair_runtime_stop_timeout"));
    const { user, onVerified } = mount(item);
    const retry = await screen.findByRole("button", { name: "Check again" });
    expect(api.getActivity).not.toHaveBeenCalled();
    expect(localStorage.getItem(repairProgressKey(item.id))).toBe(saved);
    expect(onVerified).not.toHaveBeenCalled();
    await user.click(retry);
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    expect(api.repairResumeRuntime).toHaveBeenCalledTimes(2);
    expect(api.repairResumeRuntime).toHaveBeenNthCalledWith(2, vi.mocked(api.repairResumeRuntime).mock.calls[0][0]);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(api.repairVerify).not.toHaveBeenCalled();
    expect(onVerified).toHaveBeenCalledTimes(1);
  });

  it("does not probe or finish after unmount during native resumption", async () => {
    const { item } = await savedVerifiedRepair();
    let resumed!: () => void;
    vi.mocked(api.repairResumeRuntime).mockReturnValue(new Promise<void>((resolve) => { resumed = resolve; }));
    const onVerified = vi.fn();
    const view = render(<SourceRepairReview item={item} onBusyChange={vi.fn()} onVerified={onVerified} />);
    await waitFor(() => expect(api.repairResumeRuntime).toHaveBeenCalledTimes(1));
    view.unmount();
    await act(async () => resumed());
    expect(api.getActivity).not.toHaveBeenCalled();
    expect(onVerified).not.toHaveBeenCalled();
  });

  it("preserves the prepare identity when its source check could not run", async () => {
    vi.mocked(api.repairPrepareOperation).mockRejectedValue(new Error('HTTP POST /api/repairs/prepare-current returned 409: {"error":"repair_current_check_unavailable"}'));
    const item = itemFor("memories.semantic.classification");
    const { user } = mount(item);
    const choice = await screen.findByLabelText("New memory type");
    await user.selectOptions(choice, "decision");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    expect(await screen.findByRole("status")).toHaveTextContent(i18n.t("sourceRepair.preparationUnknown"));
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.checkUnavailable"));
    expect(screen.getByRole("button", { name: "Check status" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Cancel this repair" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Prepare change" })).toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).not.toBeNull();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("keeps preparation recoverable when fresh evidence rejects the issue", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    vi.mocked(api.repairPrepareOperation).mockRejectedValue(new Error('HTTP POST /api/repairs/prepare-current returned 422: {"error":"unsupported_repair_finding"}'));
    const { user } = mount(item);
    await screen.findByLabelText("New page title");
    await user.type(screen.getByLabelText("New page title"), "Unnecessary title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    expect(await screen.findByRole("status")).toHaveTextContent(i18n.t("sourceRepair.preparationUnknown"));
    const refusal = await screen.findByRole("alert");
    expect(refusal).toHaveTextContent(i18n.t("sourceRepair.staleProposal"));
    expect(refusal).toHaveTextContent(i18n.t("sourceRepair.pendingUntilReviewed"));
    expect(screen.getByRole("button", { name: "Check status" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Cancel this repair" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Prepare change" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Apply this change" })).toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).not.toBeNull();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("does not turn missing saved entity names into an empty-selection approval", async () => {
    const item = itemFor("memories.enrichment_failures");
    const manifest = await manifestFor(item);
    writeRepairProgress({ version: 1, reviewId: item.id, checkId: "memories.enrichment_failures", occurrenceDigest: (item.payload as RepairPayload).occurrence_digest, ownerIds: item.sourceIds, phase: "prepared", manifest });
    const { user } = mount(item);
    const apply = await screen.findByRole("button", { name: "Apply this change" });
    await screen.findByText(i18n.t("sourceRepair.selectedEntitiesUnavailable"));
    expect(screen.queryByText(i18n.t("sourceRepair.entityPreviewEmpty"))).toBeNull();
    expect(apply).toBeDisabled();
    await user.click(apply);
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("reloads a prepared extraction record with resolved entity names and enables Apply", async () => {
    const item = itemFor("memories.enrichment_failures");
    const manifest = await manifestFor(item);
    const payload = item.payload as RepairPayload;
    writeRepairProgress({
      version: 1,
      reviewId: item.id,
      checkId: payload.check_id,
      occurrenceDigest: payload.occurrence_digest,
      ownerIds: item.sourceIds,
      phase: "prepared",
      manifest,
    });
    // An unfiled target queries the uncategorized scope, and the returned
    // entity must still match the target's null space/domain before it can be
    // used to render the prepared manifest's ids.
    vi.mocked(api.listEntities).mockResolvedValue([{ ...entities[0], space: null }]);
    const receipt = applyReceiptFor(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const { user } = mount(item);

    const apply = await screen.findByRole("button", { name: "Apply this change" });
    expect(await screen.findByText("Complete entity extraction and link: Same scope")).toBeVisible();
    expect(apply).toBeEnabled();
    expect(api.repairRecovery).not.toHaveBeenCalled();
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();

    await user.click(apply);
    await waitFor(() => expect(api.repairApply).toHaveBeenCalledWith({
      manifest_id: manifest.manifest_id,
      approved_manifest_digest: manifest.manifest_digest,
      approval: `apply repair ${manifest.manifest_id} ${manifest.manifest_digest}`,
    }));
  });

  it("renders actual target choices and sends the unfiled query flags without applying during prepare", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    vi.mocked(api.repairLint).mockResolvedValue(lintReport("general", (item.payload as RepairPayload).check_id));
    mockPrepared(manifest);
    const { user } = mount(item);
    expect(await screen.findByText(page.title)).toBeVisible();
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await screen.findByRole("button", { name: "Apply this change" });
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(api.repairLint).not.toHaveBeenCalled();
    expect(api.repairPrepareOperation).toHaveBeenCalledWith(expect.objectContaining({ operation_id: expect.any(String), request: expect.objectContaining({ lint_scope: { kind: "uncategorized" }, choice: expect.objectContaining({ before_title: page.title, after_title: "New title" }) }) }));
  });

  it.each([
    "pages.semantic.provenance_adequacy",
    "pages.semantic.faithfulness",
  ])("loads the single page target for read-only semantic check %s", async (checkId) => {
    const item = itemFor(checkId, page.id);
    mount(item);

    expect(await screen.findByText(page.title)).toBeVisible();
    expect(screen.getByText(page.content)).toBeVisible();
    expect(screen.getByRole("status")).toHaveTextContent(i18n.t("sourceRepair.unsupportedPending"));
    expect(screen.queryByRole("button", { name: "Prepare change" })).toBeNull();
    expect(api.getPage).toHaveBeenCalledWith(page.id);
    expect(api.getMemoryDetail).not.toHaveBeenCalled();
  });

  it("does not fetch a page for a mixed-owner semantic page finding", async () => {
    const item = { ...itemFor("pages.semantic.provenance_adequacy", page.id), sourceIds: [page.id, memory.source_id] };
    mount(item);

    expect(await screen.findByText(i18n.t("sourceRepair.unsupportedPending"))).toBeVisible();
    expect(api.getPage).not.toHaveBeenCalled();
    expect(api.getMemoryDetail).not.toHaveBeenCalled();
    expect(screen.queryByText(page.content)).toBeNull();
  });

  it("requires an exact binding before showing Apply", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    manifest.source.review_binding!.review_id = "newer-review";
    mockPrepared(manifest);
    const { user } = mount(item);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    expect(await screen.findByRole("status")).toHaveTextContent(i18n.t("sourceRepair.preparationUnknown"));
    expect(screen.queryByRole("button", { name: "Apply this change" })).toBeNull();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("guards duplicate apply clicks and keeps a stale-like server error recoverable", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    mockPrepared(manifest);
    let rejectApply!: (reason: Error) => void;
    vi.mocked(api.repairApply).mockReturnValue(new Promise((_, reject) => { rejectApply = reject; }));
    const { user } = mount(item);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    const apply = await screen.findByRole("button", { name: "Apply this change" });
    fireEvent.click(apply);
    fireEvent.click(apply);
    await waitFor(() => expect(api.repairApply).toHaveBeenCalledTimes(1));
    rejectApply(new Error("409 stale conflict after commit"));
    expect(await screen.findByText(/apply result is unknown/i)).toBeVisible();
    expect(screen.queryByText(i18n.t("sourceRepair.verificationPending"))).toBeNull();
    const recover = screen.getByRole("button", { name: "Check status" });
    expect(recover).toBeEnabled();
  });

  it("probes the normal activity route after a successful verification", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    mockPrepared(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await user.click(await screen.findByRole("button", { name: "Apply this change" }));
    await waitFor(() => expect(onVerified).toHaveBeenCalledTimes(1));
    expect(api.getActivity).toHaveBeenCalledTimes(1);
    expect(api.repairResumeRuntime).toHaveBeenCalledTimes(1);
    expect(vi.mocked(api.repairVerify).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.repairResumeRuntime).mock.invocationCallOrder[0]);
    expect(vi.mocked(api.repairResumeRuntime).mock.invocationCallOrder[0]).toBeLessThan(vi.mocked(api.getActivity).mock.invocationCallOrder[0]);
  });

  it("keeps a verified repair blocked when the normal activity route is unavailable", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    mockPrepared(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    vi.mocked(api.getActivity).mockRejectedValue(new Error("404 activity"));
    const onVerified = vi.fn();
    const { user, onBusyChange } = mount(item, onVerified);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await user.click(await screen.findByRole("button", { name: "Apply this change" }));
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.verifiedServiceUnavailable"));
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled();
    expect(onVerified).not.toHaveBeenCalled();
    expect(onBusyChange.mock.calls[onBusyChange.mock.calls.length - 1]).toEqual([true]);
    expect(api.repairApply).toHaveBeenCalledTimes(1);
    expect(api.repairVerify).toHaveBeenCalledTimes(1);
  });

  it("rechecks the normal activity route without reapplying or reverifying", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    mockPrepared(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    vi.mocked(api.getActivity).mockRejectedValueOnce(new Error("404 activity")).mockResolvedValue(activityResponse());
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await user.click(await screen.findByRole("button", { name: "Apply this change" }));
    await user.click(await screen.findByRole("button", { name: "Check again" }));
    const continueButton = await screen.findByRole("button", { name: "Continue" });
    await user.click(continueButton);
    expect(onVerified).toHaveBeenCalledTimes(1);
    expect(api.getActivity).toHaveBeenCalledTimes(2);
    expect(api.repairApply).toHaveBeenCalledTimes(1);
    expect(api.repairVerify).toHaveBeenCalledTimes(1);
  });

  it("retries verification without reapplying after a verification failure", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    mockPrepared(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockRejectedValueOnce(new Error("verification unavailable")).mockResolvedValue(verificationFor(manifest, receipt));
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await user.click(await screen.findByRole("button", { name: "Apply this change" }));
    const retry = await screen.findByRole("button", { name: "Retry verification" });
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.appliedVerificationFailed"));
    expect(screen.queryByText(i18n.t("sourceRepair.verificationPending"))).toBeNull();
    await user.click(retry);
    await waitFor(() => expect(onVerified).toHaveBeenCalledTimes(1));
    expect(api.repairApply).toHaveBeenCalledTimes(1);
    expect(api.repairVerify).toHaveBeenCalledTimes(2);
  });

  it("resumes an applying record only through an explicit recovery action", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const repair = item.payload as RepairPayload;
    writeRepairProgress({ version: 1, reviewId: item.id, checkId: repair.check_id, occurrenceDigest: repair.occurrence_digest, ownerIds: item.sourceIds, phase: "applying", manifest } as RepairProgressRecord);
    const receipt = applyReceiptFor(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const onVerified = vi.fn();
    const { user, onBusyChange } = mount(item, onVerified);
    const recover = await screen.findByRole("button", { name: "Check status" });
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenCalledWith(true);
    vi.mocked(api.repairOperationStatus).mockImplementation(async (request) => ({ manifest_id: request.manifest_id, manifest_digest: request.approved_manifest_digest, state: { phase: "indeterminate" } }));
    await user.click(recover);
    expect(api.repairApply).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Recover and apply this change" }));
    await waitFor(() => expect(onVerified).toHaveBeenCalledTimes(1));
    expect(api.repairApply).toHaveBeenCalledWith({ manifest_id: manifest.manifest_id, approved_manifest_digest: manifest.manifest_digest, approval: `apply repair ${manifest.manifest_id} ${manifest.manifest_digest}` });
  });

  it("hydrates a cold-cache applied recovery and retries verification without applying", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest, apply_receipt: receipt });
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);

    expect(await screen.findByRole("button", { name: "Retry verification" })).toBeEnabled();
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(localStorage.getItem(`wenlan.repair.progress.v1:${encodeURIComponent(item.id)}`)).not.toBeNull();

    await user.click(screen.getByRole("button", { name: "Retry verification" }));
    await waitFor(() => expect(onVerified).toHaveBeenCalledTimes(1));
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(api.repairVerify).toHaveBeenCalledTimes(1);
  });

  it("hydrates a cold-cache uncertain recovery and retries the same apply", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest, apply_receipt: null });
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);

    const recover = await screen.findByRole("button", { name: "Check status" });
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    vi.mocked(api.repairOperationStatus).mockImplementation(async (request) => ({ manifest_id: request.manifest_id, manifest_digest: request.approved_manifest_digest, state: { phase: "indeterminate" } }));
    await user.click(recover);
    expect(api.repairApply).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Recover and apply this change" }));
    await waitFor(() => expect(onVerified).toHaveBeenCalledTimes(1));
    expect(api.repairApply).toHaveBeenCalledTimes(1);
    expect(api.repairApply).toHaveBeenCalledWith({ manifest_id: manifest.manifest_id, approved_manifest_digest: manifest.manifest_digest, approval: `apply repair ${manifest.manifest_id} ${manifest.manifest_digest}` });
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
  });

  it("probes the normal activity route before continuing a saved verified record", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    const verification = verificationFor(manifest, receipt);
    writeRepairProgress({
      version: 1,
      reviewId: item.id,
      checkId: (item.payload as RepairPayload).check_id,
      occurrenceDigest: (item.payload as RepairPayload).occurrence_digest,
      ownerIds: item.sourceIds,
      phase: "verified",
      manifest,
      applyReceipt: receipt,
      verificationReceipt: verification,
    });
    vi.mocked(api.getActivity).mockRejectedValueOnce(new Error("404 activity")).mockResolvedValue(activityResponse());
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.verifiedServiceUnavailable"));
    expect(onVerified).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Check again" }));
    await user.click(await screen.findByRole("button", { name: "Continue" }));
    expect(onVerified).toHaveBeenCalledTimes(1);
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(api.repairVerify).not.toHaveBeenCalled();
    expect(api.getActivity).toHaveBeenCalledTimes(2);
  });

  it("replaces malformed local progress with an authenticated pending daemon recovery", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    localStorage.setItem(repairProgressKey(item.id), "{malformed");
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest, apply_receipt: null });
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const { user } = mount(item);
    expect(await screen.findByRole("button", { name: "Check status" })).toBeEnabled();
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(localStorage.getItem(repairProgressKey(item.id))).not.toBe("{malformed");
    vi.mocked(api.repairOperationStatus).mockImplementation(async (request) => ({ manifest_id: request.manifest_id, manifest_digest: request.approved_manifest_digest, state: { phase: "indeterminate" } }));
    await user.click(screen.getByRole("button", { name: "Check status" }));
    expect(api.repairApply).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Recover and apply this change" }));
    await waitFor(() => expect(api.repairApply).toHaveBeenCalledTimes(1));
  });

  it("replaces a local digest failure with an authenticated daemon recovery", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const localManifest = await manifestFor(item);
    localManifest.mutation = { ...localManifest.mutation, after_title: "Tampered" } as RepairManifest["mutation"];
    writeRepairProgress({ version: 1, reviewId: item.id, checkId: (item.payload as RepairPayload).check_id, occurrenceDigest: (item.payload as RepairPayload).occurrence_digest, ownerIds: item.sourceIds, phase: "prepared", manifest: localManifest });
    const durableManifest = await manifestFor(item);
    const receipt = applyReceiptFor(durableManifest);
    vi.mocked(api.repairValidateManifest).mockResolvedValueOnce(false).mockResolvedValue(true);
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest: durableManifest, apply_receipt: receipt });
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(durableManifest, receipt));
    mount(item);
    expect(await screen.findByRole("button", { name: "Retry verification" })).toBeEnabled();
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it.each(["identity", "binding"] as const)("replaces a local %s failure with an authenticated daemon recovery", async (failure) => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const localManifest = await manifestFor(item);
    if (failure === "binding") localManifest.source.review_binding!.review_id = "another-review";
    localStorage.setItem(repairProgressKey(item.id), JSON.stringify({
      version: 1,
      reviewId: failure === "identity" ? "another-review" : item.id,
      checkId: (item.payload as RepairPayload).check_id,
      occurrenceDigest: (item.payload as RepairPayload).occurrence_digest,
      ownerIds: item.sourceIds,
      phase: "applying",
      manifest: localManifest,
    }));
    const durableManifest = await manifestFor(item);
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest: durableManifest, apply_receipt: null });
    mount(item);
    expect(await screen.findByRole("button", { name: "Check status" })).toBeEnabled();
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
  });

  it("uses a normal ready flow when recovery reports no pending artifact", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const { user } = mount(item);

    expect(await screen.findByLabelText("New page title")).toBeVisible();
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    await user.type(screen.getByLabelText("New page title"), "New title");
    expect(screen.getByRole("button", { name: "Prepare change" })).toBeEnabled();
  });

  it("keeps recovery blocked after a failed recovery read and retries that read explicitly", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    vi.mocked(api.repairRecovery)
      .mockRejectedValueOnce(new Error("recovery endpoint unavailable"))
      .mockResolvedValueOnce(null);
    const { user } = mount(item);

    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.recoveryFailed"));
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    await user.click(screen.getByRole("button", { name: "Retry recovery" }));
    expect(await screen.findByLabelText("New page title")).toBeVisible();
    expect(api.repairRecovery).toHaveBeenCalledTimes(2);
  });

  it("keeps a corrupt local record blocked when daemon recovery is empty", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    localStorage.setItem(repairProgressKey(item.id), "{malformed");
    const { user } = mount(item);
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.storageFailed"));
    expect(screen.queryByRole("button", { name: "Prepare change" })).toBeNull();
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Retry recovery" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Retry recovery" }));
    await waitFor(() => expect(api.repairRecovery).toHaveBeenCalledTimes(2));
  });

  it("recovers a corrupt local record after the recovery retry succeeds", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    localStorage.setItem(repairProgressKey(item.id), "{malformed");
    vi.mocked(api.repairRecovery).mockRejectedValueOnce(new Error("daemon unavailable")).mockResolvedValueOnce({ manifest, apply_receipt: null });
    const { user } = mount(item);
    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.recoveryFailed"));
    expect(screen.getByRole("button", { name: "Retry recovery" })).toBeEnabled();
    await user.click(screen.getByRole("button", { name: "Retry recovery" }));
    expect(await screen.findByRole("button", { name: "Check status" })).toBeEnabled();
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
  });

  it.each(["binding", "digest", "receipt"] as const)("fails closed for a recovery %s mismatch", async (kind) => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    if (kind === "binding") manifest.source.review_binding!.review_id = "another-review";
    if (kind === "digest") manifest.manifest_digest = "not-a-digest";
    if (kind === "receipt") receipt.manifest_id = "another-manifest";
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest, apply_receipt: receipt });
    mount(item);

    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.recoveryFailed"));
    expect(screen.getByRole("button", { name: "Retry recovery" })).toBeEnabled();
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(localStorage.getItem(`wenlan.repair.progress.v1:${encodeURIComponent(item.id)}`)).toBeNull();
  });

  it("does not load entities while hydrating an extraction recovery", async () => {
    const item = itemFor("memories.enrichment_failures");
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    vi.mocked(api.repairRecovery).mockResolvedValue({ manifest, apply_receipt: receipt });
    mount(item);

    expect(await screen.findByRole("button", { name: "Retry verification" })).toBeEnabled();
    expect(api.listEntities).not.toHaveBeenCalled();
  });

  it("ignores a late recovery reply after unmount", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    let resolveRecovery!: (value: null) => void;
    vi.mocked(api.repairRecovery).mockReturnValue(new Promise((resolve) => {
      resolveRecovery = resolve;
    }));
    const onBusyChange = vi.fn();
    const view = render(<SourceRepairReview item={item} onBusyChange={onBusyChange} onVerified={vi.fn()} />);
    await waitFor(() => expect(api.repairRecovery).toHaveBeenCalledWith(item.id));
    const callsBeforeUnmount = onBusyChange.mock.calls.length;
    view.unmount();
    resolveRecovery(null);
    await Promise.resolve();

    expect(onBusyChange).toHaveBeenCalledTimes(callsBeforeUnmount);
    expect(api.getPage).not.toHaveBeenCalled();
    expect(localStorage.getItem(`wenlan.repair.progress.v1:${encodeURIComponent(item.id)}`)).toBeNull();
  });

  it("blocks Apply when progress storage fails", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    mockPrepared(manifest);
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => { throw new Error("storage denied"); });
    const { user } = mount(item);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await screen.findByText(/local recovery data could not be saved/i);
    expect(screen.queryByRole("button", { name: "Apply this change" })).toBeNull();
    expect(api.repairApply).not.toHaveBeenCalled();
    vi.restoreAllMocks();
  });

  it("keeps an unreadable local record blocked when its target is missing", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    localStorage.setItem(repairProgressKey(item.id), "{malformed");
    vi.mocked(api.getPage).mockResolvedValue(null);
    const onBusyChange = vi.fn();
    mount(item, vi.fn(), onBusyChange);

    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.storageFailed"));
    expect(api.getPage).toHaveBeenCalledWith(page.id);
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenCalledWith(true);
  });

  it("keeps an unreadable local record blocked after a successful target read", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    localStorage.setItem(repairProgressKey(item.id), "{malformed");
    const onBusyChange = vi.fn();
    mount(item, vi.fn(), onBusyChange);

    expect(await screen.findByRole("alert")).toHaveTextContent(i18n.t("sourceRepair.storageFailed"));
    expect(await screen.findByText(page.title)).toBeVisible();
    expect(api.getPage).toHaveBeenCalledWith(page.id);
    expect(api.repairRecovery).toHaveBeenCalledWith(item.id);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenCalledWith(true);
  });

  it.each(["applied_unverified", "verified"] as const)("recovers a saved %s record without a receipt through the same manifest", async (phase) => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const payload = item.payload as RepairPayload;
    writeRepairProgress({ version: 1, reviewId: item.id, checkId: payload.check_id,
      occurrenceDigest: payload.occurrence_digest, ownerIds: item.sourceIds, phase, manifest });
    const receipt = applyReceiptFor(manifest);
    vi.mocked(api.repairApply).mockResolvedValue(receipt);
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);
    const recover = await screen.findByRole("button", { name: "Check status" });
    expect(onVerified).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    vi.mocked(api.repairOperationStatus).mockImplementation(async (request) => ({ manifest_id: request.manifest_id, manifest_digest: request.approved_manifest_digest, state: { phase: "indeterminate" } }));
    await user.click(recover);
    expect(api.repairApply).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Recover and apply this change" }));
    await waitFor(() => expect(onVerified).toHaveBeenCalledTimes(1));
    expect(api.repairApply).toHaveBeenCalledTimes(1);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
  });

  it("keeps the manifest before-type visible after the current target changes", async () => {
    const item = itemFor("memories.semantic.classification");
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    const payload = item.payload as RepairPayload;
    writeRepairProgress({
      version: 1,
      reviewId: item.id,
      checkId: payload.check_id,
      occurrenceDigest: payload.occurrence_digest,
      ownerIds: item.sourceIds,
      phase: "applied_unverified",
      manifest,
      applyReceipt: receipt,
    });
    vi.mocked(api.getMemoryDetail).mockResolvedValue({ ...memory, memory_type: "preference" });

    mount(item);

    expect(await screen.findByText("Original type: Fact")).toBeVisible();
    expect(screen.queryByText("Current type: Preference")).toBeNull();
    expect(screen.getByText("Type: Fact → Preference")).toBeVisible();
  });

  it("does not verify or complete when the apply receipt targets another manifest", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    mockPrepared(manifest);
    vi.mocked(api.repairApply).mockResolvedValue({ ...applyReceiptFor(manifest), manifest_id: "other" });
    const onVerified = vi.fn();
    const { user } = mount(item, onVerified);
    await screen.findByText(page.title);
    await user.type(screen.getByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await user.click(await screen.findByRole("button", { name: "Apply this change" }));
    await screen.findByText(/apply receipt did not match/i);
    expect(api.repairVerify).not.toHaveBeenCalled();
    expect(onVerified).not.toHaveBeenCalled();
  });

  it("shows a missing target as blocked and never offers preparation", async () => {
    vi.mocked(api.getPage).mockResolvedValue(null);
    const item = itemFor("pages.duplicate_active_titles", page.id);
    mount(item);
    expect(await screen.findByRole("alert")).toHaveTextContent("target is no longer available");
    expect(screen.queryByRole("button", { name: "Prepare change" })).toBeNull();
  });

  it("renders same-scope extraction choices and permits an explicit empty selection", async () => {
    const scopedMemory = { ...memory, space: "work" };
    vi.mocked(api.getMemoryDetail).mockResolvedValue(scopedMemory);
    vi.mocked(api.listEntities).mockResolvedValue(entities);
    const item = itemFor("memories.enrichment_failures", scopedMemory.source_id);
    mount(item);
    expect(await screen.findByText("Same scope")).toBeVisible();
    expect(screen.queryByText("Other scope")).toBeNull();
    expect(screen.getByText(/leave the selection empty/i)).toBeVisible();
  });
});


function preparationFor(item: RepairItem): RepairPreparationRecord {
  const payload = item.payload as RepairPayload;
  return { version: 1, reviewId: item.id, checkId: payload.check_id, occurrenceDigest: payload.occurrence_digest, ownerIds: item.sourceIds,
    operation: { operation_id: "aaaaaaaa-aaaa-4aaa-aaaa-aaaaaaaaaaaa", request: { lint_scope: { kind: "uncategorized" }, choice: { kind: "rename_page_title", review_id: item.id, page_id: item.sourceIds[0], before_title: page.title, after_title: "New title" } } } };
}

function readyPreparation(operationId: string, manifest: RepairManifest) {
  return { operation_id: operationId, state: { phase: "ready" as const, manifest, operation: { manifest_id: manifest.manifest_id, manifest_digest: manifest.manifest_digest, state: { phase: "prepared" as const } } } };
}

describe("durable repair operation controls", () => {
  it("persists before prepare and recovers a lost response after remount without preparing again", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    let sent: RepairPrepareOperationRequest | undefined;
    vi.mocked(api.repairPrepareOperation).mockImplementation(async (request) => {
      sent = request;
      expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).toContain(request.operation_id);
      throw new Error("response lost");
    });
    const first = render(<SourceRepairReview item={item} onBusyChange={vi.fn()} onVerified={vi.fn()} />);
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    await screen.findByText(i18n.t("sourceRepair.preparationUnknown"));
    expect(api.repairApply).not.toHaveBeenCalled();
    first.unmount();
    vi.mocked(api.repairPrepareOperationStatus).mockResolvedValue(readyPreparation(sent!.operation_id, manifest));
    mount(item);
    expect(await screen.findByRole("button", { name: "Apply this change" })).toBeEnabled();
    expect(api.repairPrepareOperationStatus).toHaveBeenCalledWith(sent);
    expect(api.repairPrepareOperation).toHaveBeenCalledTimes(1);
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).toBeNull();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("only reports cancellation after a durable acknowledgement and never completes the review", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const saved = preparationFor(item);
    writeRepairPreparation(saved);
    vi.mocked(api.repairPrepareOperationStatus).mockResolvedValue({ operation_id: saved.operation.operation_id, state: { phase: "not_started" } });
    vi.mocked(api.repairPrepareOperationCancel)
      .mockResolvedValueOnce({ operation_id: saved.operation.operation_id, state: { phase: "in_progress" } })
      .mockResolvedValueOnce({ operation_id: saved.operation.operation_id, state: { phase: "cancelled", cancelled_at: 123 } });
    const { user, onVerified, onBusyChange } = mount(item);
    await user.click(await screen.findByRole("button", { name: "Cancel this repair" }));
    expect(await screen.findByText(i18n.t("sourceRepair.preparationInProgress"))).toBeVisible();
    expect(screen.queryByText(i18n.t("sourceRepair.cancelledChange"))).toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).not.toBeNull();
    await user.click(screen.getByRole("button", { name: "Cancel this repair" }));
    expect(await screen.findByText(i18n.t("sourceRepair.cancelledChange"))).toBeVisible();
    expect(onVerified).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenLastCalledWith(false);
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).toBeNull();
  });

  it("recognizes a cancelled saved manifest before offering apply", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    writeRepairProgress({ ...preparationFor(item), phase: "prepared", manifest });
    vi.mocked(api.repairOperationStatus).mockResolvedValue({ manifest_id: manifest.manifest_id, manifest_digest: manifest.manifest_digest, state: { phase: "cancelled", cancelled_at: 123 } });
    const { onVerified } = mount(item);
    expect(await screen.findByText(i18n.t("sourceRepair.cancelledChange"))).toBeVisible();
    expect(screen.queryByRole("button", { name: "Apply this change" })).toBeNull();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(onVerified).not.toHaveBeenCalled();
    expect(localStorage.getItem(repairProgressKey(item.id))).toBeNull();
  });

  it("uses the returned apply receipt for verification instead of applying again", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const receipt = applyReceiptFor(manifest);
    writeRepairProgress({ ...preparationFor(item), phase: "applying", manifest });
    vi.mocked(api.repairOperationStatus).mockResolvedValue({ manifest_id: manifest.manifest_id, manifest_digest: manifest.manifest_digest, state: { phase: "applied_unverified", apply_receipt: receipt } });
    vi.mocked(api.repairVerify).mockResolvedValue(verificationFor(manifest, receipt));
    const { user } = mount(item);
    await user.click(await screen.findByRole("button", { name: "Check status" }));
    expect(api.repairApply).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Retry verification" }));
    await waitFor(() => expect(api.repairVerify).toHaveBeenCalledTimes(1));
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("keeps a mismatched cancellation response unresolved", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const saved = preparationFor(item);
    writeRepairPreparation(saved);
    vi.mocked(api.repairPrepareOperationStatus).mockResolvedValue({ operation_id: saved.operation.operation_id, state: { phase: "interrupted" } });
    vi.mocked(api.repairPrepareOperationCancel).mockResolvedValue({ operation_id: "bbbbbbbb-bbbb-4bbb-bbbb-bbbbbbbbbbbb", state: { phase: "cancelled", cancelled_at: 123 } });
    const { user, onVerified } = mount(item);
    await user.click(await screen.findByRole("button", { name: "Cancel this repair" }));
    expect(screen.queryByText(i18n.t("sourceRepair.cancelledChange"))).toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).not.toBeNull();
    expect(onVerified).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("hands a cancelled preparation with saved progress to manifest recovery without clearing it", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const saved = preparationFor(item);
    writeRepairPreparation(saved);
    const payload = item.payload as RepairPayload;
    writeRepairProgress({ version: 1, reviewId: item.id, checkId: payload.check_id,
      occurrenceDigest: payload.occurrence_digest, ownerIds: item.sourceIds, phase: "prepared", manifest });
    vi.mocked(api.repairPrepareOperationStatus).mockResolvedValue(
      { operation_id: saved.operation.operation_id, state: { phase: "cancelled", cancelled_at: 123 } });
    mount(item);
    expect(await screen.findByRole("button", { name: "Apply this change" })).toBeEnabled();
    expect(screen.queryByText(i18n.t("sourceRepair.cancelledChange"))).toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).toBeNull();
    expect(localStorage.getItem(repairProgressKey(item.id))).not.toBeNull();
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
  });

  it("does not send a fresh prepare over a saved progress record", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const { user } = mount(item);
    await user.type(await screen.findByLabelText("New page title"), "New title");
    const payload = item.payload as RepairPayload;
    writeRepairProgress({ version: 1, reviewId: item.id, checkId: payload.check_id,
      occurrenceDigest: payload.occurrence_digest, ownerIds: item.sourceIds, phase: "prepared", manifest });
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    expect(await screen.findByRole("button", { name: "Apply this change" })).toBeEnabled();
    expect(api.repairPrepareOperation).not.toHaveBeenCalled();
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(localStorage.getItem(repairProgressKey(item.id))).not.toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).toBeNull();
  });

  it("continues a not-started preparation to ready with the same UUID and one send", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    const saved = preparationFor(item);
    writeRepairPreparation(saved);
    vi.mocked(api.repairPrepareOperationStatus).mockResolvedValue(
      { operation_id: saved.operation.operation_id, state: { phase: "not_started" } });
    vi.mocked(api.repairPrepareOperation).mockImplementation(async (request) =>
      readyPreparation(request.operation_id, manifest));
    const { user } = mount(item);
    await user.click(await screen.findByRole("button", { name: "Continue preparing" }));
    expect(await screen.findByRole("button", { name: "Apply this change" })).toBeEnabled();
    expect(api.repairPrepareOperation).toHaveBeenCalledTimes(1);
    expect(api.repairPrepareOperation).toHaveBeenCalledWith(saved.operation);
    expect(vi.mocked(api.repairPrepareOperation).mock.calls[0][0].operation_id)
      .toBe(saved.operation.operation_id);
    expect(api.repairApply).not.toHaveBeenCalled();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).toBeNull();
    expect(localStorage.getItem(repairProgressKey(item.id))).not.toBeNull();
  });

  it("ignores a late prepare reply after the component switches to another proposal", async () => {
    const item = itemFor("pages.duplicate_active_titles", page.id);
    const manifest = await manifestFor(item);
    let resolve!: (value: ReturnType<typeof readyPreparation>) => void;
    let sent: RepairPrepareOperationRequest | undefined;
    vi.mocked(api.repairPrepareOperation).mockImplementation((request) => { sent = request; return new Promise((done) => { resolve = done; }); });
    const view = render(<SourceRepairReview item={item} onBusyChange={vi.fn()} onVerified={vi.fn()} />);
    const user = userEvent.setup();
    await user.type(await screen.findByLabelText("New page title"), "New title");
    await user.click(screen.getByRole("button", { name: "Prepare change" }));
    const other = { ...itemFor("pages.duplicate_active_titles", "page-other"), id: "other-review" };
    vi.mocked(api.getPage).mockResolvedValue({ ...page, id: "page-other", title: "Other page" });
    view.rerender(<SourceRepairReview item={other} onBusyChange={vi.fn()} onVerified={vi.fn()} />);
    await screen.findByText("Other page");
    await act(async () => resolve(readyPreparation(sent!.operation_id, manifest)));
    expect(screen.queryByRole("button", { name: "Apply this change" })).toBeNull();
    expect(screen.getByRole("button", { name: "Prepare change" })).toBeEnabled();
    expect(localStorage.getItem(repairProgressKey(other.id))).toBeNull();
    expect(localStorage.getItem(`wenlan.repair.prepare.v1:${encodeURIComponent(item.id)}`)).not.toBeNull();
  });
});
