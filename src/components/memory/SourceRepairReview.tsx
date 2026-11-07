// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import {
  daemonErrorMessage,
  getActivity,
  getMemoryDetail,
  getPage,
  listEntities,
  repairApply,
  repairLint,
  repairPrepareOperation,
  repairPrepareOperationStatus,
  repairPrepareOperationCancel,
  repairOperationStatus,
  repairCancel,
  repairRecovery,
  repairResumeRuntime,
  repairVerify,
  type Entity,
  type MemoryItem,
  type MemoryType,
  type Page,
  type RefinementPayload,
  type ActivityResponse,
} from "../../lib/tauri";
import type { ReviewItem } from "./useReviewQueue";
import "./SourceRepairReview.css";
import {
  applyRequestForManifest,
  classifyApplyFailure,
  clearRepairProgress,
  validateRepairOperationStatus,
  readRepairProgress,
  validateRepairApplyReceipt,
  validateRepairManifestBinding,
  validateRepairManifestDigest,
  validateRepairVerificationReceipt,
  writeRepairProgress,
  type RepairProgressPhase,
  type RepairProgressRecord,
  type RepairReviewIdentity,
} from "../../lib/repairWorkflow";
import type {
  CurrentRepairChoice,
  RepairLintReport,
  RepairLintScope,
  RepairManifest,
  RepairApplyReceipt,
  RepairVerificationReceipt,
  RepairOperationStatus,
  RepairPrepareOperationStatus,
} from "../../lib/repairTypes";
import { readRepairPreparation, writeRepairPreparation, clearRepairPreparation, validateRepairPreparationStatus, type RepairPreparationRecord } from "../../lib/repairPreparation";
import { loadRelationRepairSources, type RelationRepairSources } from "../../lib/relationRepair";

type RepairReviewItem = Extract<ReviewItem, { kind: "refinement" }>;
type RepairPayload = Extract<RefinementPayload, { action: "lint_repair_review" }>;

const MEMORY_TYPES: MemoryType[] = [
  "identity",
  "preference",
  "decision",
  "lesson",
  "gotcha",
  "fact",
];

const CHECK_CLASSIFICATION = "memories.semantic.classification";
const CHECK_DUPLICATE_TITLES = "pages.duplicate_active_titles";
const CHECK_ENRICHMENT = "memories.enrichment_failures";
const CHECK_RELATIONS = "kg.semantic.entity_relations";
const CHECK_PROVENANCE = "pages.semantic.provenance_adequacy";
const CHECK_FAITHFULNESS = "pages.semantic.faithfulness";

const paneStyle: React.CSSProperties = {
  fontFamily: "var(--mem-font-body)",
  fontSize: 14,
  lineHeight: 1.65,
  color: "var(--mem-text)",
  overflowWrap: "anywhere",
  minWidth: 0,
};

const labelStyle: React.CSSProperties = {
  display: "grid",
  gap: 8,
  color: "var(--mem-text-secondary)",
  fontSize: "var(--mem-text-meta)",
};

const buttonStyle: React.CSSProperties = {
  fontFamily: "var(--mem-font-body)",
  fontSize: "var(--mem-text-control)",
  borderRadius: "var(--mem-r-8)",
  padding: "8px 15px",
  cursor: "pointer",
  border: "1px solid var(--mem-border)",
  backgroundColor: "var(--mem-surface)",
  color: "var(--mem-text)",
  justifySelf: "end",
};

function repairPayload(item: RepairReviewItem): RepairPayload | null {
  return item.payload?.action === "lint_repair_review" ? item.payload : null;
}

function targetIdFor(item: RepairReviewItem): string | null {
  if (item.sourceIds.length !== 1) return null;
  const id = item.sourceIds[0];
  return typeof id === "string" && id.trim().length > 0 ? id : null;
}

function scopeFor(space: string | null | undefined): RepairLintScope {
  const value = space?.trim();
  return value ? { kind: "registered", space: value } : { kind: "uncategorized" };
}

function lintQueryForScope(profile: "general" | "deep", scope: RepairLintScope) {
  return {
    profile,
    ...(scope.kind === "registered"
      ? { space: scope.space }
      : scope.kind === "uncategorized"
        ? { space: "uncategorized" }
        : {}),
    external_egress: false,
    // Agent-assist reports contain pending work rather than the provider
    // packet required to bind a semantic repair.
    agent_assist: false,
  };
}

function targetSpace(target: MemoryItem | Page | null): string | null {
  const value = target?.space ?? target?.domain ?? null;
  return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
}

function sameScope(entity: Entity, space: string | null): boolean {
  const entitySpace = entity.space ?? entity.domain ?? null;
  return (typeof entitySpace === "string" ? entitySpace.trim() : entitySpace) === space;
}

function isMemoryType(value: string | null | undefined): value is MemoryType {
  return !!value && MEMORY_TYPES.includes(value as MemoryType);
}

function diagnostic(value: unknown): string {
  if (value instanceof Error && value.message) return value.message;
  if (typeof value === "string" && value.trim()) return value;
  return "unknown error";
}

function parseRepairRecovery(value: unknown): {
  manifest: RepairManifest;
  applyReceipt: RepairApplyReceipt | null;
} | null {
  if (value === null) return null;
  if (typeof value !== "object" || value === null) {
    throw new Error("repair recovery returned a malformed response");
  }
  const candidate = value as { manifest?: unknown; apply_receipt?: unknown };
  if (typeof candidate.manifest !== "object" || candidate.manifest === null ||
      (candidate.apply_receipt !== null && typeof candidate.apply_receipt !== "object")) {
    throw new Error("repair recovery returned a malformed response");
  }
  return {
    manifest: candidate.manifest as RepairManifest,
    applyReceipt: candidate.apply_receipt as RepairApplyReceipt | null,
  };
}

function reportReady(report: RepairLintReport | undefined, profile: "general" | "deep"): report is RepairLintReport {
  return report?.profile === profile && report.complete === true;
}

function hasApplicableCheck(report: RepairLintReport | undefined, checkId: string): report is RepairLintReport {
  return report?.profile === "deep" && !!report.checks.some(
    (check) => check.check_id === checkId && (check.outcome === "pass" || check.outcome === "finding"),
  );
}

function needsDeepVerification(manifest: RepairManifest, checkId: string | null): boolean {
  const policy = manifest.post_assertions?.verification_policy;
  if (policy?.kind === "applicable_checks") {
    return policy.required_deep_check_ids.length > 0;
  }
  return checkId === CHECK_CLASSIFICATION || checkId === CHECK_RELATIONS;
}

export interface SourceRepairReviewProps {
  item: RepairReviewItem;
  onBusyChange: (busy: boolean) => void;
  onVerified: () => void;
  technicalDetails?: React.ReactNode;
  actionHost?: HTMLElement | null;
}

type FlowState =
  | "inspecting"
  | "ready"
  | "preparing"
  | "uncertain_prepare"
  | "cancelled"
  | "prepared"
  | "applying"
  | "uncertain_apply"
  | "verifying"
  | "applied_unverified"
  | "restoring_service"
  | "verified_service_unavailable"
  | "verified"
  | "error";

type ErrorKind = "target" | "unsupported" | "stale" | "storage" | "recovery" | "check_unavailable" | "prepare" | "pre_apply" | "verify" | "unknown_apply" | "service_unavailable" | "operation";

const ACTIVITY_STATES: ActivityResponse["state"][] = [
  "up_to_date",
  "organizing",
  "waiting_for_idle",
  "blocked",
  "unknown",
];

function isActivityResponse(value: unknown): value is ActivityResponse {
  if (typeof value !== "object" || value === null) return false;
  const state = (value as { state?: unknown }).state;
  return typeof state === "string" && ACTIVITY_STATES.includes(state as ActivityResponse["state"]);
}

export default function SourceRepairReview({
  item,
  onBusyChange,
  onVerified,
  technicalDetails,
  actionHost,
}: SourceRepairReviewProps) {
  const { t } = useTranslation();
  const payload = repairPayload(item);
  const targetId = targetIdFor(item);
  const checkId = payload?.check_id ?? null;
  const supported = item.action === "lint_repair_review" &&
    (checkId === CHECK_CLASSIFICATION || checkId === CHECK_DUPLICATE_TITLES || checkId === CHECK_ENRICHMENT || checkId === CHECK_RELATIONS);
  // These semantic page findings are deliberately read-only here. They may
  // show the page that needs review, but this UI has no claim writer yet.
  const readOnlyPageCheck = item.action === "lint_repair_review" &&
    (checkId === CHECK_PROVENANCE || checkId === CHECK_FAITHFULNESS) &&
    item.sourceIds.length === 1 && targetId !== null;
  const identity = useMemo<RepairReviewIdentity | null>(() =>
    payload && checkId
      ? {
          reviewId: item.id,
          checkId,
          occurrenceDigest: payload.occurrence_digest,
          ownerIds: [...item.sourceIds],
        }
      : null, [checkId, item.id, item.sourceIds, payload]);

  const [flow, setFlow] = useState<FlowState>(supported || readOnlyPageCheck ? "inspecting" : "error");
  const [errorKind, setErrorKind] = useState<ErrorKind | null>(supported || readOnlyPageCheck ? null : "unsupported");
  const [errorDetail, setErrorDetail] = useState<string | null>(null);
  const [target, setTarget] = useState<MemoryItem | Page | null>(null);
  const [entities, setEntities] = useState<Entity[]>([]);
  const [manifest, setManifest] = useState<RepairManifest | null>(null);
  const [applyReceipt, setApplyReceipt] = useState<RepairApplyReceipt | null>(null);
  const [verificationReceipt, setVerificationReceipt] = useState<RepairVerificationReceipt | null>(null);
  const [selectedType, setSelectedType] = useState<MemoryType | "">("");
  const [newTitle, setNewTitle] = useState("");
  const [relationSources, setRelationSources] = useState<RelationRepairSources | null>(null);
  const [relationAction, setRelationAction] = useState<"add" | "retire">("add");
  const [relationFrom, setRelationFrom] = useState("");
  const [relationTo, setRelationTo] = useState("");
  const [relationType, setRelationType] = useState("");
  const [relationSource, setRelationSource] = useState("");
  const [retireRelationId, setRetireRelationId] = useState("");
  const [selectedEntities, setSelectedEntities] = useState<Set<string>>(new Set());
  const [busy, setBusy] = useState(false);
  const [recoveryAttempt, setRecoveryAttempt] = useState(0);
  const [recoveryRetryable, setRecoveryRetryable] = useState(false);
  const [preparationGate, setPreparationGate] = useState<"checking" | "ready" | "held">("checking");
  const [preparation, setPreparation] = useState<RepairPreparationRecord | null>(null);
  const [preparationPhase, setPreparationPhase] = useState<RepairPrepareOperationStatus["state"]["phase"] | "unknown">("unknown");
  const [operationPhase, setOperationPhase] = useState<RepairOperationStatus["state"]["phase"] | "unknown">("unknown");
  // A persisted prepare send refused by the daemon keeps its exact
  // preparation record for query/cancel and shows the refusal reason visibly.
  const [prepareRefusal, setPrepareRefusal] = useState<"stale" | "check_unavailable" | null>(null);
  const operationEpochRef = useRef(0);
  const requestInFlightRef = useRef(false);
  const retainedBusyRef = useRef(false);
  const approvalPreflightRef = useRef(false);
  const mountedRef = useRef(true);
  const reportedVerifiedRef = useRef(false);
  // Distinct from `flow === "verified"`: set only after native runtime
  // resumption AND a fresh Activity probe both succeed for the current
  // identity. Continue/finishVerified gate on this, not on `flow` alone, so a
  // synchronous pre-probe render can never make the button clickable.
  const serviceReadyRef = useRef(false);
  const serviceEpochRef = useRef(0);

  const setBusyState = (next: boolean) => {
    if (!mountedRef.current) return;
    setBusy(next);
    onBusyChange(next);
  };

  const beginRequest = (): boolean => {
    if (requestInFlightRef.current) return false;
    requestInFlightRef.current = true;
    retainedBusyRef.current = false;
    setBusyState(true);
    return true;
  };

  const finishRequest = (retainBusy = false) => {
    requestInFlightRef.current = false;
    retainedBusyRef.current = retainBusy;
    if (!retainBusy) setBusyState(false);
    else if (mountedRef.current) {
      // The host remains navigation-locked while recovery is unresolved, but
      // the local recovery control itself must be usable after the request
      // settles.
      setBusy(false);
      onBusyChange(true);
    }
  };

  const setError = (kind: ErrorKind, detail: unknown, retainBusy = false) => {
    if (!mountedRef.current) return;
    setErrorKind(kind);
    setErrorDetail(diagnostic(detail));
    finishRequest(retainBusy);
  };

  const currentMemory = target && "source_id" in target ? target : null;
  const currentPage = target && "id" in target ? target : null;
  const currentType = currentMemory?.memory_type ?? null;
  const currentSpace = targetSpace(target);

  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      // The host dialog is being unmounted, so late replies must not publish
      // state. It also owns the final navigation decision.
    };
  }, []);

  const progressForOperation = (nextManifest: RepairManifest, status: RepairOperationStatus): RepairProgressRecord | null => {
    if (!identity || !validateRepairOperationStatus(status, nextManifest)) throw new Error("repair operation status is not bound to this manifest");
    const state = status.state;
    if (state.phase === "cancelled") return null;
    const phase: RepairProgressPhase = state.phase === "prepared" ? "prepared" : state.phase === "verified" ? "verified" : state.phase === "applied_unverified" ? "applied_unverified" : "applying";
    return { ...identity, version: 1, phase, manifest: nextManifest,
      ...((state.phase === "applied_unverified" || state.phase === "verified") ? { applyReceipt: state.apply_receipt } : {}),
      ...(state.phase === "verified" ? { verificationReceipt: state.verification_receipt } : {}),
    };
  };

  // Resolve a saved client ID before the older manifest recovery path can
  // expose editable inputs. A lost prepare response is not permission to start over.
  useEffect(() => {
    let stopped = false;
    operationEpochRef.current += 1;
    setPreparationGate("checking");
    setPreparation(null);
    setPreparationPhase("unknown");
    setOperationPhase("unknown");
    setPrepareRefusal(null);
    if (!supported || !identity) { setPreparationGate("ready"); return; }
    requestInFlightRef.current = true;
    setBusyState(true);
    const savedPreparation = readRepairPreparation(identity);
    if (savedPreparation.error) {
      setPreparationGate("held"); setFlow("error"); setErrorKind("storage");
      setErrorDetail(diagnostic(savedPreparation.error)); setRecoveryRetryable(true); finishRequest(true);
      return;
    }
    if (!savedPreparation.record) { setPreparationGate("ready"); return; }
    const saved = savedPreparation.record;
    setPreparation(saved);
    setFlow("uncertain_prepare");
    setPreparationGate("held");
    const load = async () => {
      try {
        const result = await repairPrepareOperationStatus(saved.operation);
        if (stopped || !mountedRef.current) return;
        if (!validateRepairPreparationStatus(result, saved.operation, identity)) throw new Error("invalid preparation recovery response");
        const existing = readRepairProgress(identity);
        if (existing.error) throw existing.error;
        if (result.state.phase === "ready") {
          const nextManifest = result.state.manifest;
          if (existing.record && (existing.record.manifest.manifest_id !== nextManifest.manifest_id || existing.record.manifest.manifest_digest !== nextManifest.manifest_digest)) throw new Error("preparation conflicts with saved manifest recovery");
          if (!(await validateRepairManifestDigest(nextManifest))) throw new Error("invalid recovered manifest digest");
          if (stopped || !mountedRef.current) return;
          const record = progressForOperation(nextManifest, result.state.operation);
          if (record) writeRepairProgress(record); else clearRepairProgress(identity.reviewId);
          clearRepairPreparation(identity.reviewId);
          setPreparation(null);
          if (record) setPreparationGate("ready");
          else { setFlow("cancelled"); setErrorKind(null); finishRequest(false); }
        } else {
          if (result.state.phase === "cancelled" && existing.record) {
            // An acknowledged cancellation meets an unrelated saved manifest:
            // drop only the preparation and let existing progress recovery own
            // the UI. The saved manifest is never cleared or marked cancelled.
            clearRepairPreparation(identity.reviewId);
            if (stopped || !mountedRef.current) return;
            setPreparation(null); setPrepareRefusal(null); setPreparationGate("ready");
            return;
          }
          if (existing.record) throw new Error("unresolved preparation conflicts with saved manifest recovery");
          setPreparationPhase(result.state.phase);
          setPrepareRefusal(null);
          if (result.state.phase === "cancelled") {
            clearRepairPreparation(identity.reviewId); setPreparation(null); setFlow("cancelled"); setErrorKind(null); finishRequest(false);
          } else { setFlow("uncertain_prepare"); finishRequest(true); }
        }
      } catch (error) {
        if (stopped || !mountedRef.current) return;
        setFlow("uncertain_prepare"); setErrorKind("operation"); setErrorDetail(diagnostic(error)); finishRequest(true);
      }
    };
    void load();
    return () => { stopped = true; operationEpochRef.current += 1; };
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [identity?.reviewId, identity?.occurrenceDigest, supported, recoveryAttempt]);

  useEffect(() => {
    if (preparationGate !== "ready") return;
    if ((!supported && !readOnlyPageCheck) || !identity || !payload) {
      setFlow("error");
      setErrorKind("unsupported");
      setBusyState(false);
      return;
    }
    let cancelled = false;
    requestInFlightRef.current = true;
    retainedBusyRef.current = false;
    setBusyState(true);
    setFlow("inspecting");
    setErrorKind(null);
    setErrorDetail(null);
    setTarget(null);
    setEntities([]);
    setManifest(null);
    setApplyReceipt(null);
    setVerificationReceipt(null);
    setSelectedEntities(new Set());
    setSelectedType("");
    setNewTitle("");
    setRelationSources(null);
    setRelationAction("add");
    setRelationFrom(""); setRelationTo(""); setRelationType("");
    setRelationSource(""); setRetireRelationId("");
    setRecoveryRetryable(false);
    serviceEpochRef.current += 1;
    // Reset for this identity/request before any branch below can set it.
    serviceReadyRef.current = false;

    const saved = supported ? readRepairProgress(identity) : { record: null, error: null };
    // Capture this before any asynchronous target read. React state from the
    // render that started the request is stale inside the catch handler; the
    // immutable local read result is the authority for whether recovery must
    // stay blocked.
    let localRecoveryBlocked = saved.error != null;
    let localRecoveryDetail: unknown = saved.error;
    let verifiedRecoveryReady = false;
    if (saved.error) {
      setErrorKind("storage");
      setErrorDetail(diagnostic(saved.error));
      setRecoveryRetryable(true);
    } else if (saved.record) {
      const receipt = saved.record.applyReceipt;
      const hasValidReceipt = receipt != null && validateRepairApplyReceipt(receipt, saved.record.manifest);
      const verification = saved.record.verificationReceipt;
      const hasValidVerification = hasValidReceipt && verification != null &&
        validateRepairVerificationReceipt(verification, saved.record.manifest, receipt);
      setManifest(saved.record.manifest);
      setApplyReceipt(hasValidReceipt ? receipt : null);
      setVerificationReceipt(hasValidVerification ? verification : null);
      if (saved.record.phase !== "prepared") {
        verifiedRecoveryReady = saved.record.phase === "verified" && hasValidVerification;
        // A verified record still needs native resumption and a fresh
        // Activity probe before Continue is real; do not claim "verified"
        // until that async work actually succeeds (see probeNormalService).
        setFlow(verifiedRecoveryReady ? "restoring_service" : hasValidReceipt ? "applied_unverified" : "uncertain_apply");
        if (!hasValidReceipt) setErrorKind("operation");
        retainedBusyRef.current = true;
      } else {
        setFlow("prepared");
      }
    }

    let recoveryRecord = saved.record;
    const load = async () => {
      let recoveryReadFailed = false;
      try {
        if (saved.record) {
          try {
            const validDigest = await validateRepairManifestDigest(saved.record.manifest);
            if (cancelled || !mountedRef.current) return;
            if (!validDigest) throw new Error("saved repair manifest digest is invalid");
          } catch (error) {
            if (cancelled || !mountedRef.current) return;
            // The local artifact is no longer trusted. Keep the session locked
            // while asking the daemon for its durable copy of this proposal.
            localRecoveryBlocked = true;
            localRecoveryDetail = error;
            setManifest(null);
            setApplyReceipt(null);
            setVerificationReceipt(null);
            setRecoveryRetryable(true);
          }
        }

        // A local read or integrity failure may hide an already-started repair.
        // Ask the daemon for the exact durable artifact before considering any
        // editable state. A failed read must never be mistaken for no recovery.
        const shouldReadDurableRecovery = supported && (localRecoveryBlocked || (!saved.error && !saved.record));
        if (shouldReadDurableRecovery) {
          let recovered: ReturnType<typeof parseRepairRecovery>;
          try {
            recovered = parseRepairRecovery(await repairRecovery(identity.reviewId));
            if (cancelled || !mountedRef.current) return;
          } catch (error) {
            if (cancelled || !mountedRef.current) return;
            setFlow("error");
            setErrorKind("recovery");
            setErrorDetail(diagnostic(localRecoveryDetail ? new Error(`${diagnostic(error)}; local recovery data: ${diagnostic(localRecoveryDetail)}`) : error));
            setRecoveryRetryable(true);
            finishRequest(true);
            return;
          }
          if (recovered) {
            recoveryReadFailed = true;
            const binding = validateRepairManifestBinding(recovered.manifest, identity);
            if (!binding.ok) throw new Error("repair recovery manifest is not bound to this proposal");
            const validDigest = await validateRepairManifestDigest(recovered.manifest);
            if (cancelled || !mountedRef.current) return;
            if (!validDigest) throw new Error("repair recovery manifest digest is invalid");
            if (recovered.applyReceipt && !validateRepairApplyReceipt(recovered.applyReceipt, recovered.manifest)) {
              throw new Error("repair recovery apply receipt did not match the manifest");
            }
            const hydratedRecord: RepairProgressRecord = {
              ...identity,
              version: 1,
              phase: recovered.applyReceipt ? "applied_unverified" : "applying",
              manifest: recovered.manifest,
              ...(recovered.applyReceipt ? { applyReceipt: recovered.applyReceipt } : {}),
            };
            try {
              writeRepairProgress(hydratedRecord);
            } catch (error) {
              if (cancelled || !mountedRef.current) return;
              setManifest(null);
              setApplyReceipt(null);
              setFlow("error");
              setErrorKind("recovery");
              setErrorDetail(diagnostic(error));
              finishRequest(true);
              return;
            }
            recoveryRecord = hydratedRecord;
            recoveryReadFailed = false;
            localRecoveryBlocked = false;
            localRecoveryDetail = null;
            setRecoveryRetryable(false);
            setManifest(recovered.manifest);
            setApplyReceipt(recovered.applyReceipt);
            setFlow(recovered.applyReceipt ? "applied_unverified" : "uncertain_apply");
            if (!recovered.applyReceipt) setErrorKind("operation");
            retainedBusyRef.current = true;
          } else {
            // Keep the storage failure visible when the daemon has no copy.
            // The local evidence is not discarded and no replacement proposal
            // may be prepared from the current source.
            recoveryRecord = null;
          }
        }

        if (recoveryRecord?.phase === "prepared" && !localRecoveryBlocked) {
          const nextManifest = recoveryRecord.manifest;
          recoveryReadFailed = true;
          const status = await repairOperationStatus(applyRequestForManifest(nextManifest));
          if (cancelled || !mountedRef.current) return;
          if (!validateRepairOperationStatus(status, nextManifest)) throw new Error("invalid saved repair operation status");
          const nextRecord = progressForOperation(nextManifest, status);
          if (!nextRecord) {
            clearRepairProgress(identity.reviewId); setManifest(null); setFlow("cancelled"); setErrorKind(null); finishRequest(false); return;
          }
          writeRepairProgress(nextRecord);
          recoveryRecord = nextRecord;
          setOperationPhase(status.state.phase);
          setApplyReceipt(nextRecord.applyReceipt ?? null);
          setVerificationReceipt(nextRecord.verificationReceipt ?? null);
          // An in_progress/indeterminate manifest status keeps the conservative
          // applying phase (no new mutations under an unknown lock owner) and
          // still explains the operation next to the explicit recovery actions.
          if (nextRecord.phase === "applying") setErrorKind("operation");
          verifiedRecoveryReady = nextRecord.phase === "verified";
          setFlow(nextRecord.phase === "prepared" ? "prepared" : nextRecord.phase === "verified" ? "restoring_service" : nextRecord.applyReceipt ? "applied_unverified" : "uncertain_apply");
          recoveryReadFailed = false;
        }
        if (cancelled || !mountedRef.current) return;
        if (!targetId && checkId !== CHECK_RELATIONS) throw new Error("the proposal has no single target");
        let loadedTarget: MemoryItem | Page | null = null;
        if (checkId === CHECK_RELATIONS) {
          const sources = await loadRelationRepairSources(identity.ownerIds);
          if (cancelled || !mountedRef.current) return;
          setRelationSources(sources);
        } else if ((checkId === CHECK_DUPLICATE_TITLES || readOnlyPageCheck) && targetId) {
          loadedTarget = await getPage(targetId);
          if (!loadedTarget || loadedTarget.id !== targetId) throw new Error("page target is no longer available");
        } else if (targetId) {
          loadedTarget = await getMemoryDetail(targetId);
          if (!loadedTarget || loadedTarget.source_id !== targetId) throw new Error("memory target is no longer available");
        }
        if (cancelled || !mountedRef.current) return;
        setTarget(loadedTarget);
        // Recovery already carries the exact entity ids in its manifest. Do
        // not replace that authoritative artifact with a fresh editable list.
        if (checkId === CHECK_ENRICHMENT && (!recoveryRecord || recoveryRecord.phase === "prepared")) {
          const loadedEntities = await listEntities(undefined, targetSpace(loadedTarget) ?? "uncategorized");
          if (cancelled || !mountedRef.current) return;
          setEntities(loadedEntities.filter((entity) => sameScope(entity, targetSpace(loadedTarget))));
        }
        if (cancelled || !mountedRef.current) return;
        if (!supported) {
          setFlow("ready");
          finishRequest(false);
          return;
        }
        if (localRecoveryBlocked) {
          // An unreadable or invalid local record may describe an already
          // started repair. A null durable response does not make it safe to
          // prepare a replacement.
          setFlow("error");
          setErrorKind("storage");
          setErrorDetail(diagnostic(localRecoveryDetail ?? "local repair progress is unavailable"));
          setRecoveryRetryable(true);
          finishRequest(true);
          return;
        }
        if (verifiedRecoveryReady && recoveryRecord) {
          // hasValidVerification guarantees this record carries a bound
          // verification receipt; the guard below is defensive, not expected.
          const resumeVerification = recoveryRecord.verificationReceipt;
          if (!resumeVerification) {
            setFlow("error");
            setErrorKind("recovery");
            setErrorDetail("verified recovery is missing its verification receipt");
            setRecoveryRetryable(true);
            finishRequest(true);
            return;
          }
          if (!(await probeNormalService(recoveryRecord.manifest, resumeVerification, () => cancelled))) return;
          if (cancelled || !mountedRef.current) return;
          setErrorKind(null);
          setErrorDetail(null);
          setFlow("verified");
          finishRequest(false);
          return;
        }
        // A saved uncertain/applied record keeps the host busy; no mutation or
        // verification is started on remount. Recovery is always explicit.
        const recovering = recoveryRecord &&
          (recoveryRecord.phase === "applying" || recoveryRecord.phase === "applied_unverified" || recoveryRecord.phase === "verified");
        if (!recovering) {
          setFlow(recoveryRecord ? "prepared" : "ready");
          retainedBusyRef.current = false;
          finishRequest(false);
        } else {
          finishRequest(true);
        }
      } catch (error) {
        if (cancelled || !mountedRef.current) return;
        if (recoveryReadFailed) {
          setFlow("error");
          setErrorKind("recovery");
          setErrorDetail(diagnostic(localRecoveryDetail ? new Error(`${diagnostic(error)}; local recovery data: ${diagnostic(localRecoveryDetail)}`) : error));
          setRecoveryRetryable(true);
          finishRequest(true);
          return;
        }
        if (localRecoveryBlocked) {
          setFlow("error");
          setErrorKind("storage");
          setErrorDetail(diagnostic(localRecoveryDetail ?? error));
          setRecoveryRetryable(true);
          finishRequest(true);
          return;
        }
        const recovering = recoveryRecord != null &&
          (recoveryRecord.phase === "applying" || recoveryRecord.phase === "applied_unverified" || recoveryRecord.phase === "verified");
        if (recovering && recoveryRecord) {
          // Keep the exact saved manifest recoverable even if its target detail
          // disappeared during a remount. Never unlock or reinterpret an apply
          // that may already have committed.
          const receipt = recoveryRecord.applyReceipt;
          setFlow(receipt && validateRepairApplyReceipt(receipt, recoveryRecord.manifest)
            ? "applied_unverified" : "uncertain_apply");
          setErrorKind("target");
          setErrorDetail(diagnostic(error));
          finishRequest(true);
          return;
        }
        setFlow("error");
        setErrorKind("target");
        setErrorDetail(diagnostic(error));
        finishRequest(false);
      }
    };
    void load();
    return () => {
      cancelled = true;
      serviceEpochRef.current += 1;
      requestInFlightRef.current = false;
    };
  // The keyed host component remounts for a new proposal; these are the
  // identity fields needed to prevent a late reply crossing that boundary.
  // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [checkId, identity?.occurrenceDigest, identity?.reviewId, readOnlyPageCheck, recoveryAttempt, supported, targetId, preparationGate]);

  const writeProgress = (record: RepairProgressRecord): boolean => {
    try {
      writeRepairProgress(record);
      return true;
    } catch (error) {
      if (mountedRef.current) {
        setErrorKind("storage");
        setErrorDetail(diagnostic(error));
      }
      return false;
    }
  };

  const baseRecord = (nextManifest: RepairManifest, phase: RepairProgressPhase): RepairProgressRecord => ({
    version: 1,
    reviewId: identity?.reviewId ?? item.id,
    checkId: identity?.checkId ?? checkId ?? "",
    occurrenceDigest: identity?.occurrenceDigest ?? payload?.occurrence_digest ?? "",
    ownerIds: [...(identity?.ownerIds ?? item.sourceIds)],
    phase,
    manifest: nextManifest,
  });

  const setWorkflowError = (kind: ErrorKind, detail: unknown) => {
    setFlow("error");
    setError(kind, detail, false);
  };

  // Takes the frozen manifest and authenticated verification receipt as
  // explicit arguments rather than reading component state, so a stale
  // setter never substitutes a different approval than the one the caller
  // just validated.
  const probeNormalService = async (
    approvedManifest: RepairManifest,
    approvedVerification: RepairVerificationReceipt,
    isCancelled?: () => boolean,
  ): Promise<boolean> => {
    const epoch = serviceEpochRef.current;
    const current = () => mountedRef.current && serviceEpochRef.current === epoch && !isCancelled?.();
    if (!current()) return false;
    serviceReadyRef.current = false;
    setFlow("restoring_service");
    try {
      // Native measures process identity and only restarts positively owned
      // services; it resolves once lifecycle handoff is finished, which is
      // not yet proof the daemon answers requests again.
      await repairResumeRuntime({
        apply: applyRequestForManifest(approvedManifest),
        verification_receipt_digest: approvedVerification.receipt_digest,
      });
      if (!current()) return false;
      const activity = await getActivity();
      if (!current()) return false;
      if (!isActivityResponse(activity)) {
        throw new Error("the normal activity route returned a malformed response");
      }
      serviceReadyRef.current = true;
      return true;
    } catch (error) {
      if (!current()) return false;
      setFlow("verified_service_unavailable");
      setErrorKind("service_unavailable");
      setErrorDetail(diagnostic(error));
      // Keep the queue item anchored while the service is unavailable. The
      // check-again control remains local and usable after the probe settles.
      finishRequest(true);
      return false;
    }
  };

  const consumeOperationStatus = async (nextManifest: RepairManifest, status: RepairOperationStatus, epoch: number) => {
    if (!identity || !validateRepairOperationStatus(status, nextManifest) || !validateRepairManifestBinding(nextManifest, identity).ok) throw new Error("repair status does not match this proposal");
    if (!mountedRef.current || operationEpochRef.current !== epoch) return;
    const record = progressForOperation(nextManifest, status);
    if (!record) {
      clearRepairProgress(identity.reviewId);
      clearRepairPreparation(identity.reviewId);
      setPreparation(null); setPrepareRefusal(null); setManifest(null); setApplyReceipt(null); setVerificationReceipt(null);
      setFlow("cancelled"); setErrorKind(null); setErrorDetail(null); finishRequest(false);
      return;
    }
    writeRepairProgress(record);
    clearRepairPreparation(identity.reviewId);
    setPrepareRefusal(null);
    setPreparation(null); setManifest(nextManifest); setApplyReceipt(record.applyReceipt ?? null);
    setVerificationReceipt(record.verificationReceipt ?? null); setOperationPhase(status.state.phase);
    setErrorKind(null); setErrorDetail(null);
    switch (record.phase) {
      case "prepared": setFlow("prepared"); finishRequest(false); break;
      case "applying": setFlow("uncertain_apply"); setErrorKind("operation"); finishRequest(true); break;
      case "applied_unverified": setFlow("applied_unverified"); finishRequest(true); break;
      case "verified":
        if (record.verificationReceipt && await probeNormalService(nextManifest, record.verificationReceipt, () => operationEpochRef.current !== epoch)) {
          if (!mountedRef.current || operationEpochRef.current !== epoch) return;
          setFlow("verified"); finishRequest(false);
        }
        break;
    }
  };

  const consumePreparationStatus = async (saved: RepairPreparationRecord, result: unknown, epoch: number) => {
    if (!identity || !validateRepairPreparationStatus(result, saved.operation, identity)) throw new Error("invalid repair preparation status");
    if (!mountedRef.current || operationEpochRef.current !== epoch) return;
    if (result.state.phase === "ready") {
      const existing = readRepairProgress(identity);
      if (existing.error) throw existing.error;
      if (existing.record && (existing.record.manifest.manifest_id !== result.state.manifest.manifest_id || existing.record.manifest.manifest_digest !== result.state.manifest.manifest_digest)) throw new Error("preparation conflicts with saved manifest recovery");
      if (!(await validateRepairManifestDigest(result.state.manifest))) throw new Error("invalid prepared manifest digest");
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      await consumeOperationStatus(result.state.manifest, result.state.operation, epoch);
      return;
    }
    const existing = readRepairProgress(identity);
    if (existing.error) throw existing.error;
    if (existing.record) {
      if (result.state.phase === "cancelled") {
        // An acknowledged cancellation meets an unrelated saved manifest:
        // drop only the preparation and hand off to existing progress
        // recovery. The saved manifest is never cleared or marked cancelled.
        clearRepairPreparation(identity.reviewId);
        if (!mountedRef.current || operationEpochRef.current !== epoch) return;
        setPreparation(null); setPrepareRefusal(null);
        setPreparationGate("checking");
        setRecoveryAttempt((attempt) => attempt + 1);
        return;
      }
      throw new Error("unresolved preparation conflicts with saved manifest recovery");
    }
    setPreparationPhase(result.state.phase);
    setPrepareRefusal(null);
    setErrorKind(null); setErrorDetail(null);
    if (result.state.phase === "cancelled") {
      clearRepairPreparation(identity.reviewId);
      setPreparation(null); setFlow("cancelled"); finishRequest(false);
    } else {
      setPreparation(saved); setFlow("uncertain_prepare"); finishRequest(true);
    }
  };

  const checkPreparation = async (action: "status" | "cancel" | "retry") => {
    const saved = preparation;
    if (!saved || !beginRequest()) return;
    const epoch = operationEpochRef.current;
    try {
      const result = action === "cancel" ? await repairPrepareOperationCancel(saved.operation)
        : action === "retry" ? await repairPrepareOperation(saved.operation)
        : await repairPrepareOperationStatus(saved.operation);
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      await consumePreparationStatus(saved, result, epoch);
    } catch (error) {
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      setFlow("uncertain_prepare"); setErrorKind("operation"); setErrorDetail(diagnostic(error)); finishRequest(true);
    }
  };

  const checkManifestOperation = async (cancel = false) => {
    const nextManifest = manifest;
    if (!nextManifest || !beginRequest()) return;
    const epoch = operationEpochRef.current;
    try {
      const request = applyRequestForManifest(nextManifest);
      const status = cancel ? await repairCancel(request) : await repairOperationStatus(request);
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      await consumeOperationStatus(nextManifest, status, epoch);
    } catch (error) {
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      setOperationPhase("unknown"); setFlow("uncertain_apply"); setErrorKind("operation"); setErrorDetail(diagnostic(error)); finishRequest(true);
    }
  };

  const prepare = async () => {
    // Check all local prerequisites before taking the host navigation lock.
    if (!identity || !payload || !supported || (!target && !relationSources) || !beginRequest()) return;
    const epoch = operationEpochRef.current;
    // A fresh prepare must never overwrite a saved manifest. An unreadable
    // progress record fails closed to existing recovery.
    const guardedProgress = readRepairProgress(identity);
    if (guardedProgress.error) {
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      setFlow("error"); setErrorKind("storage"); setErrorDetail(diagnostic(guardedProgress.error));
      setRecoveryRetryable(true); finishRequest(true); return;
    }
    if (guardedProgress.record) {
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      // Preserve the saved manifest and let existing progress recovery own
      // the UI instead of sending a replacement prepare.
      setPreparationGate("checking");
      setRecoveryAttempt((attempt) => attempt + 1);
      return;
    }
    let savedPreparation: RepairPreparationRecord | null = null;
    let preparationPersisted = false;
    setFlow("preparing");
    setErrorKind(null);
    setErrorDetail(null);
    setPrepareRefusal(null);
    try {
      // A relation can intentionally cross spaces. Fresh preparation checks
      // every queued owner; do not infer its scope from one endpoint.
      const lintScope: RepairLintScope = checkId === CHECK_RELATIONS ? { kind: "global" } : scopeFor(targetSpace(target));
      let choice: CurrentRepairChoice;
      if (checkId === CHECK_CLASSIFICATION) {
        if (!currentMemory || !selectedType || selectedType === currentType) {
          throw new Error("choose a different memory type");
        }
        choice = { kind: "reclassify_memory", review_id: item.id, memory_id: currentMemory.source_id, after_memory_type: selectedType };
      } else if (checkId === CHECK_DUPLICATE_TITLES) {
        if (!currentPage || !newTitle.trim() || newTitle.trim() === currentPage.title.trim()) {
          throw new Error("enter a new page title");
        }
        choice = {
          kind: "rename_page_title",
          review_id: item.id,
          page_id: currentPage.id,
          before_title: currentPage.title,
          after_title: newTitle.trim(),
        };
      } else if (checkId === CHECK_ENRICHMENT) {
        if (!currentMemory) throw new Error("memory target is unavailable");
        const entityIds = [...selectedEntities]
          .filter((id) => entities.some((entity) => entity.id === id))
          .sort();
        choice = {
          kind: "complete_entity_extraction",
          review_id: item.id,
          memory_id: currentMemory.source_id,
          // The daemon's contract permits an empty, explicit selection.
          entity_ids: entityIds,
        };
      } else if (checkId === CHECK_RELATIONS && relationSources) {
        choice = { kind: "entity_relation", selection: { review_id: item.id, choice:
          relationAction === "retire"
            ? { kind: "retire", relation_id: retireRelationId }
            : { kind: "add", from_entity: relationFrom, to_entity: relationTo,
                relation_type: relationType, ...(relationSource ? { source_memory_id: relationSource } : {}) },
        } };
      } else {
        throw new Error("unsupported source repair check");
      }
      savedPreparation = { ...identity, version: 1, operation: {
        operation_id: crypto.randomUUID(), request: { lint_scope: lintScope, choice },
      } };
      // A storage failure must happen before any request is sent.
      writeRepairPreparation(savedPreparation);
      preparationPersisted = true;
      setPreparation(savedPreparation);
      setPreparationPhase("unknown");
      const result = await repairPrepareOperation(savedPreparation.operation);
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      await consumePreparationStatus(savedPreparation, result, epoch);
    } catch (error) {
      if (!mountedRef.current || operationEpochRef.current !== epoch) return;
      // Only a successfully persisted ID may have been sent; preserve it for
      // lookup. A daemon refusal keeps the exact record and its query/cancel
      // actions, and maps stale/unavailable codes to visible diagnostic copy.
      if (savedPreparation && preparationPersisted) {
        const code = daemonErrorMessage(error);
        const staleRefusal = code === "repair_current_finding_missing" ||
          code === "repair_target_stale" || code === "unsupported_repair_finding";
        setPrepareRefusal(staleRefusal ? "stale" : code === "repair_current_check_unavailable" ? "check_unavailable" : null);
        setPreparation(savedPreparation); setFlow("uncertain_prepare"); setErrorKind("operation");
        setErrorDetail(diagnostic(error)); finishRequest(true); return;
      }
      if (savedPreparation && !preparationPersisted) {
        setWorkflowError("storage", error); setRecoveryRetryable(true); return;
      }
      const code = daemonErrorMessage(error);
      const noCurrentFinding = code === "repair_current_finding_missing" ||
        code === "repair_target_stale" || code === "unsupported_repair_finding";
      setWorkflowError(noCurrentFinding ? "stale" : code === "repair_current_check_unavailable" ? "check_unavailable" : "prepare", error);
    }
  };

  const verify = async (nextManifest: RepairManifest, receipt: RepairApplyReceipt) => {
    if (!identity || !payload) return;
    if (!validateRepairApplyReceipt(receipt, nextManifest)) {
      setWorkflowError("verify", "the apply receipt did not match the prepared change");
      finishRequest(true);
      return;
    }
    setFlow("verifying");
    try {
      // Recovery may outlive the source detail, so use the scope frozen in
      // the manifest rather than re-deriving it from a current target.
      const lintScope = nextManifest.source.lint_scope;
      const general = await repairLint(lintQueryForScope("general", lintScope));
      const deep = needsDeepVerification(nextManifest, checkId)
        ? await repairLint(lintQueryForScope("deep", lintScope))
        : undefined;
      const requiredDeepCheckIds = nextManifest.post_assertions?.verification_policy?.kind === "applicable_checks"
        ? nextManifest.post_assertions.verification_policy.required_deep_check_ids
        : deep ? [nextManifest.source.check_id] : [];
      if (!reportReady(general, "general") ||
          (deep && (!hasApplicableCheck(deep, requiredDeepCheckIds[0] ?? CHECK_CLASSIFICATION) ||
            requiredDeepCheckIds.some((required) => !hasApplicableCheck(deep, required))))) {
        throw new Error("the fresh lint report is incomplete");
      }
      const verified = await repairVerify({
        manifest_id: nextManifest.manifest_id,
        manifest_digest: nextManifest.manifest_digest,
        apply_receipt_digest: receipt.receipt_digest,
        general_report: general,
        ...(deep ? { deep_report: deep } : {}),
      });
      if (!validateRepairVerificationReceipt(verified, nextManifest, receipt)) {
        throw new Error("the verification receipt did not match the applied change");
      }
      const record = baseRecord(nextManifest, "verified");
      record.applyReceipt = receipt;
      record.verificationReceipt = verified;
      if (!writeProgress(record)) {
        setWorkflowError("storage", "the verification receipt could not be saved");
        finishRequest(true);
        return;
      }
      if (!mountedRef.current) return;
      setVerificationReceipt(verified);
      setFlow("restoring_service");
      if (!(await probeNormalService(nextManifest, verified))) return;
      if (!mountedRef.current) return;
      setFlow("verified");
      finishRequest(false);
      if (!reportedVerifiedRef.current) {
        reportedVerifiedRef.current = true;
        onVerified();
      }
    } catch (error) {
      if (!mountedRef.current) return;
      setFlow("applied_unverified");
      setErrorKind("verify");
      setErrorDetail(diagnostic(error));
      finishRequest(true);
    }
  };

  const apply = async () => {
    if (!manifest || !identity) return;
    if (flow === "prepared" && previewEntitiesMissing) return;
    // Reserve the approval click synchronously before the asynchronous digest
    // check so two same-tick clicks cannot both reach the daemon.
    if (approvalPreflightRef.current) return;
    approvalPreflightRef.current = true;
    // Lock the host before the asynchronous digest check. A dialog
    // navigation during this final local preflight must not race the apply.
    setBusyState(true);
    // Re-check the binding and signed payload at the final approval boundary.
    // A persisted or mutated preview must never be turned into a different
    // daemon request by the UI.
    const binding = validateRepairManifestBinding(manifest, identity);
    if (!binding.ok) {
      approvalPreflightRef.current = false;
      setWorkflowError("stale", binding.reason);
      return;
    }
    let digestValid = false;
    try {
      digestValid = await validateRepairManifestDigest(manifest);
    } catch (error) {
      approvalPreflightRef.current = false;
      setWorkflowError("stale", error);
      return;
    }
    if (!digestValid) {
      approvalPreflightRef.current = false;
      setWorkflowError("stale", "the prepared manifest digest is invalid");
      return;
    }
    if (!mountedRef.current) {
      approvalPreflightRef.current = false;
      return;
    }
    if (!beginRequest()) {
      approvalPreflightRef.current = false;
      return;
    }
    approvalPreflightRef.current = false;
    setErrorKind(null);
    setErrorDetail(null);
    setFlow("applying");
    const applyingRecord = baseRecord(manifest, "applying");
    if (!writeProgress(applyingRecord)) {
      setFlow("prepared");
      setErrorKind("storage");
      finishRequest(false);
      return;
    }
    try {
      const receipt = await repairApply(applyRequestForManifest(manifest));
      setApplyReceipt(receipt);
      if (!validateRepairApplyReceipt(receipt, manifest)) {
        // The endpoint may have committed before returning a malformed or
        // mismatched receipt. Preserve the same applying record and require
        // explicit retry of that exact manifest.
        setApplyReceipt(null);
        const uncertainRecord = baseRecord(manifest, "applying");
        setFlow("uncertain_apply");
        if (!writeProgress(uncertainRecord)) {
          setErrorKind("storage");
          setErrorDetail("the apply receipt did not match the prepared change and recovery could not be saved");
        } else {
          setErrorKind("unknown_apply");
          setErrorDetail("the apply receipt did not match the prepared change");
        }
        finishRequest(true);
        return;
      }
      const appliedRecord = baseRecord(manifest, "applied_unverified");
      appliedRecord.applyReceipt = receipt;
      if (!writeProgress(appliedRecord)) {
        setFlow("applied_unverified");
        finishRequest(true);
        return;
      }
      await verify(manifest, receipt);
    } catch (error) {
      if (!mountedRef.current) return;
      const kind = classifyApplyFailure(error);
      if (kind === "pre_apply") {
        // The daemon explicitly refused before mutation; make the same
        // manifest retryable and keep the proposal pending.
        const retryRecord = baseRecord(manifest, "prepared");
        if (writeProgress(retryRecord)) setFlow("prepared");
        else setFlow("error");
        setErrorKind("pre_apply");
        setErrorDetail(diagnostic(error));
        finishRequest(false);
      } else {
        // Applying began, so this manifest is retained forever for explicit
        // recovery. Never prepare a replacement after this point.
        setFlow("uncertain_apply");
        setErrorKind("unknown_apply");
        setErrorDetail(diagnostic(error));
        finishRequest(true);
      }
    }
  };

  const recover = async () => {
    if (!manifest) return;
    if (flow === "uncertain_apply") {
      await checkManifestOperation();
      return;
    }
    if (flow === "applied_unverified" && applyReceipt) {
      if (!beginRequest()) return;
      await verify(manifest, applyReceipt);
    }
  };

  const retryNormalService = async () => {
    // Freeze the approval locally before the async probe; component state
    // could otherwise change out from under an in-flight retry.
    const nextManifest = manifest;
    const receipt = applyReceipt;
    const verification = verificationReceipt;
    if (busy || !nextManifest || !receipt || !verification ||
        !validateRepairApplyReceipt(receipt, nextManifest) ||
        !validateRepairVerificationReceipt(verification, nextManifest, receipt) ||
        !beginRequest()) return;
    if (!(await probeNormalService(nextManifest, verification))) return;
    if (!mountedRef.current) return;
    setErrorKind(null);
    setErrorDetail(null);
    setFlow("verified");
    finishRequest(false);
  };

  const retryRecoveryRead = () => {
    if (busy || !mountedRef.current) return;
    // Recovery failures keep the host locked. Reserve the next read
    // synchronously so a repeated click cannot start two reads.
    setBusyState(true);
    setRecoveryAttempt((attempt) => attempt + 1);
  };

  const finishVerified = () => {
    if (!manifest || !applyReceipt || !verificationReceipt || reportedVerifiedRef.current) return;
    if (!mountedRef.current || busy || !serviceReadyRef.current) return;
    if (!validateRepairVerificationReceipt(verificationReceipt, manifest, applyReceipt)) return;
    reportedVerifiedRef.current = true;
    onVerified();
  };

  const mutationPreview = manifest?.mutation;
  const manifestBeforeMemoryType = mutationPreview?.kind === "reclassify_memory"
    ? mutationPreview.before_memory_type
    : undefined;
  const sourceMemoryType = manifestBeforeMemoryType !== undefined ? manifestBeforeMemoryType : currentType;
  const boundApplyReceipt = !!(manifest && applyReceipt && validateRepairApplyReceipt(applyReceipt, manifest));
  const uncategorizedLabel = t("sourceRepair.uncategorized");
  const editable = supported && (flow === "ready" ||
    (flow === "error" && (errorKind === "prepare" || errorKind === "pre_apply" || errorKind === "check_unavailable")));
  const previewEntities = mutationPreview?.kind === "complete_entity_extraction"
    ? mutationPreview.entity_ids.map((id) => entities.find((entity) => entity.id === id)?.name ?? null).filter((name): name is string => name != null)
    : [];
  const previewEntitiesMissing = mutationPreview?.kind === "complete_entity_extraction" &&
    previewEntities.length !== mutationPreview.entity_ids.length;
  const relationName = (id: string) => relationSources?.entities.find((entity) => entity.id === id)?.name;
  const relationLabel = (id: string) => {
    const edge = relationSources?.relations.find((relation) => relation.id === id);
    return edge ? `${relationName(edge.from_entity)} → ${relationName(edge.to_entity)} · ${edge.relation_type}` : null;
  };
  const relationChange = mutationPreview?.kind === "entity_relation" ? mutationPreview.change : null;
  const relationTarget = manifest?.target.kind === "entity_relation" ? manifest.target : null;
  const relationPreviewMissing = !!relationTarget && (!relationName(relationTarget.from_entity) ||
    !relationName(relationTarget.to_entity) || (relationChange?.kind === "add" &&
      relationChange.retire_relation_ids.some((id) => !relationLabel(id))) ||
    (relationChange?.kind === "retire" && !relationLabel(relationTarget.relation_id)));
  const relationChoiceReady = relationAction === "retire"
    ? !!relationSources?.relations.some((edge) => edge.id === retireRelationId)
    : !!relationFrom && !!relationTo && relationFrom !== relationTo && /^[a-z][a-z0-9_]*$/.test(relationType);

  const actions = <>
      {flow === "uncertain_prepare" && preparation && <>
        <button type="button" style={buttonStyle} disabled={busy} onClick={() => void checkPreparation("status")}>{t("sourceRepair.checkOperation")}</button>
        <button type="button" style={buttonStyle} disabled={busy} onClick={() => void checkPreparation("cancel")}>{t("sourceRepair.cancelChange")}</button>
        {preparationPhase === "not_started" && <button type="button" style={buttonStyle} disabled={busy} onClick={() => void checkPreparation("retry")}>{t("sourceRepair.retryPreparation")}</button>}
      </>}
      {flow === "cancelled" && <button type="button" style={buttonStyle} disabled={busy} onClick={() => { setPreparationGate("checking"); setRecoveryAttempt((old) => old + 1); }}>{t("sourceRepair.chooseAgain")}</button>}
      {(flow === "prepared" || (flow === "error" && errorKind === "target" && manifest)) && <button type="button" style={buttonStyle} disabled={busy} onClick={() => void checkManifestOperation(true)}>{t("sourceRepair.cancelChange")}</button>}
      {flow === "uncertain_apply" && <button type="button" style={buttonStyle} disabled={busy} onClick={() => void recover()}>{t("sourceRepair.checkOperation")}</button>}
      {flow === "uncertain_apply" && operationPhase === "indeterminate" && <button type="button" style={buttonStyle} disabled={busy} onClick={() => void apply()}>{t("sourceRepair.recoverApply")}</button>}
      {flow === "applied_unverified" && applyReceipt && <button type="button" style={buttonStyle} disabled={busy} onClick={() => void recover()}>{t("sourceRepair.retryVerification")}</button>}
      {flow === "verified_service_unavailable" && <button type="button" style={buttonStyle} disabled={busy} onClick={() => void retryNormalService()}>{t("sourceRepair.checkAgain")}</button>}
      {flow === "verified" && <button type="button" style={buttonStyle} disabled={busy || !serviceReadyRef.current} onClick={finishVerified}>{t("sourceRepair.continueVerified")}</button>}
      {flow === "prepared" && <button type="button" style={{ ...buttonStyle, backgroundColor: "var(--mem-accent-indigo)", color: "var(--mem-bg)", fontWeight: 600 }} disabled={busy || errorKind === "storage" || previewEntitiesMissing || relationPreviewMissing} onClick={() => void apply()}>{t("sourceRepair.applyChange")}</button>}
      {editable && <button type="button" style={{ ...buttonStyle, backgroundColor: "var(--mem-accent-indigo)", borderColor: "var(--mem-accent-indigo)", color: "var(--mem-bg)" }} disabled={busy || (checkId === CHECK_RELATIONS && !relationChoiceReady)} onClick={() => void prepare()}>{t("sourceRepair.prepareChange")}</button>}
  </>;

  const diagnosticDetails = (
    <details className="source-repair-details">
      <summary>{t("sourceRepair.showDetails")}</summary>
      {errorDetail && <pre>{errorDetail}</pre>}
      {manifest && <pre>{JSON.stringify({ manifest_id: manifest.manifest_id, manifest_digest: manifest.manifest_digest, check_id: manifest.source.check_id, target: manifest.target }, null, 2)}</pre>}
      {technicalDetails}
    </details>
  );

  const recoveryRetryAction = (
    <button type="button" style={buttonStyle} disabled={busy} onClick={retryRecoveryRead}>
      {t("sourceRepair.retryRecovery")}
    </button>
  );

  if ((!supported && !readOnlyPageCheck) || !payload || !identity) {
    return <div className="source-repair"><p role="status" style={paneStyle}>{t("sourceRepair.unsupportedPending")}</p>{diagnosticDetails}</div>;
  }

  if (flow === "inspecting") {
    return <div role="status" style={paneStyle}>{t("sourceRepair.inspecting")}</div>;
  }

  if (flow === "uncertain_prepare" || flow === "cancelled") {
    const copy = flow === "cancelled" ? "sourceRepair.cancelledChange" : preparationPhase === "in_progress" ? "sourceRepair.preparationInProgress" : preparationPhase === "interrupted" ? "sourceRepair.preparationInterrupted" : preparationPhase === "not_started" ? "sourceRepair.preparationNotStarted" : "sourceRepair.preparationUnknown";
    return <div className="source-repair" style={paneStyle}>
      <p role="status" style={{ margin: 0 }}>{t(copy)}</p>
      {flow === "uncertain_prepare" && prepareRefusal === "stale" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.staleProposal")}</p>
          <p>{t("sourceRepair.pendingUntilReviewed")}</p>
        </div>
      )}
      {flow === "uncertain_prepare" && prepareRefusal === "check_unavailable" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.checkUnavailable")}</p>
        </div>
      )}
      {actionHost ? createPortal(actions, actionHost) : actions}
      {errorDetail && diagnosticDetails}
    </div>;
  }

  if (flow === "error" && errorKind === "target") {
    return <div role="alert" className="source-repair-notice" style={paneStyle}>
      <p>{t("sourceRepair.targetMissing")}</p>
      {actionHost ? createPortal(actions, actionHost) : actions}
      {diagnosticDetails}
    </div>;
  }

  if (flow === "error" && errorKind === "recovery") {
    return <div role="alert" className="source-repair-notice" style={paneStyle}>
      <p>{t("sourceRepair.recoveryFailed")}</p>
      {actionHost ? createPortal(recoveryRetryAction, actionHost) : recoveryRetryAction}
      {diagnosticDetails}
    </div>;
  }

  return (
    <div className="source-repair" style={paneStyle}>
      {readOnlyPageCheck && (
        <div role="status" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.unsupportedPending")}</p>
        </div>
      )}
      {errorKind === "target" && (flow === "uncertain_apply" || flow === "applied_unverified") && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.targetMissing")}</p>
        </div>
      )}
      {errorKind === "stale" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.staleProposal")}</p>
          <p>{t("sourceRepair.pendingUntilReviewed")}</p>
        </div>
      )}
      {errorKind === "storage" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.storageFailed")}</p>
          {recoveryRetryable && (actionHost ? createPortal(recoveryRetryAction, actionHost) : recoveryRetryAction)}
        </div>
      )}
      {errorKind === "prepare" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.prepareFailed")}</p>
        </div>
      )}
      {errorKind === "check_unavailable" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.checkUnavailable")}</p>
        </div>
      )}
      {errorKind === "pre_apply" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.notApplied")}</p>
        </div>
      )}
      {errorKind === "operation" && (
        <div role="status" className="source-repair-notice" style={paneStyle}>
          <p>{t(operationPhase === "in_progress" ? "sourceRepair.operationInProgress" : operationPhase === "indeterminate" ? "sourceRepair.operationIndeterminate" : "sourceRepair.operationUnknown")}</p>
        </div>
      )}
      {errorKind === "unknown_apply" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.unknownApply")}</p>
          <p>{t("sourceRepair.recoverSameChange")}</p>
        </div>
      )}
      {errorKind === "service_unavailable" && flow === "verified_service_unavailable" && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.verifiedServiceUnavailable")}</p>
          {busy && <p role="status">{t("sourceRepair.checkingService")}</p>}
        </div>
      )}
      {boundApplyReceipt && (flow === "verifying" || flow === "applied_unverified") && (
        <div role={errorKind === "verify" && flow === "applied_unverified" ? "alert" : "status"} className="source-repair-notice" style={paneStyle}>
          <p>{t(errorKind === "verify" && flow === "applied_unverified"
            ? "sourceRepair.appliedVerificationFailed"
            : "sourceRepair.verificationPending")}</p>
        </div>
      )}
      {errorKind === "verify" && !boundApplyReceipt && (
        <div role="alert" className="source-repair-notice" style={paneStyle}>
          <p>{t("sourceRepair.verifyFailed")}</p>
        </div>
      )}

      {currentMemory && (
        <div className="source-repair-source">
          <p style={{ margin: "0 0 6px", fontWeight: 600 }}>{currentMemory.title || t("sourceRepair.untitledMemory")}</p>
          <p className="source-repair-content">{currentMemory.content}</p>
          {checkId === CHECK_CLASSIFICATION && <p style={{ margin: "10px 0 0", color: "var(--mem-text-secondary)" }}>
            {t(manifestBeforeMemoryType !== undefined ? "sourceRepair.originalMemoryType" : "sourceRepair.currentMemoryType", {
              type: isMemoryType(sourceMemoryType)
                ? t(`importView.typeLabels.${sourceMemoryType}`)
                : t("sourceRepair.unclassified"),
            })}
          </p>}
        </div>
      )}
      {currentPage && (
        <div className="source-repair-source">
          <p style={{ margin: "0 0 6px", fontWeight: 600 }}>{currentPage.title}</p>
          <p className="source-repair-content">{currentPage.content}</p>
        </div>
      )}

      {relationSources && <div className="source-repair-source">
        <p style={{ margin: "0 0 6px", fontWeight: 600 }}>{relationSources.entities.map((entity) => entity.name).join(" · ")}</p>
        {relationSources.memories.map((memory) => <div key={memory.source_id} style={{ marginTop: 12 }}>
          <p style={{ margin: "0 0 4px", fontWeight: 600 }}>{memory.title || t("sourceRepair.untitledMemory")}</p>
          <p className="source-repair-content">{memory.content}</p>
        </div>)}
        {relationSources.relations.length > 0 && <ul style={{ marginBottom: 0 }}>
          {relationSources.relations.map((edge) => <li key={edge.id}>{relationLabel(edge.id)}</li>)}
        </ul>}
      </div>}

      {editable && checkId === CHECK_RELATIONS && relationSources && <fieldset className="source-repair-entities" disabled={busy}>
        <legend>{t("sourceRepair.relationChange")}</legend>
        <label style={labelStyle}>{t("sourceRepair.relationAction")}
          <select aria-label={t("sourceRepair.relationAction")} value={relationAction} onChange={(event) => setRelationAction(event.target.value as "add" | "retire")}>
            <option value="add">{t("sourceRepair.relationAdd")}</option>
            <option value="retire" disabled={relationSources.relations.length === 0}>{t("sourceRepair.relationRetire")}</option>
          </select>
        </label>
        {relationAction === "retire" ? <label style={labelStyle}>{t("sourceRepair.relationToRetire")}
          <select aria-label={t("sourceRepair.relationToRetire")} value={retireRelationId} onChange={(event) => setRetireRelationId(event.target.value)}>
            <option value="">{t("sourceRepair.relationChoose")}</option>
            {relationSources.relations.map((edge) => <option key={edge.id} value={edge.id}>{relationLabel(edge.id)}</option>)}
          </select>
        </label> : <>
          <div className="source-repair-relation-endpoints">
          <label style={labelStyle}>{t("sourceRepair.relationFrom")}
            <select aria-label={t("sourceRepair.relationFrom")} value={relationFrom} onChange={(event) => setRelationFrom(event.target.value)}>
              <option value="">{t("sourceRepair.relationChoose")}</option>
              {relationSources.entities.map((entity) => <option key={entity.id} value={entity.id} disabled={entity.id === relationTo}>{entity.name}</option>)}
            </select>
          </label>
          <label style={labelStyle}>{t("sourceRepair.relationTo")}
            <select aria-label={t("sourceRepair.relationTo")} value={relationTo} onChange={(event) => setRelationTo(event.target.value)}>
              <option value="">{t("sourceRepair.relationChoose")}</option>
              {relationSources.entities.map((entity) => <option key={entity.id} value={entity.id} disabled={entity.id === relationFrom}>{entity.name}</option>)}
            </select>
          </label>
          </div>
          <label style={labelStyle}>{t("sourceRepair.relationType")}
            <input aria-label={t("sourceRepair.relationType")} value={relationType} onChange={(event) => setRelationType(event.target.value)} aria-describedby="relation-type-help" />
            <span id="relation-type-help">{t("sourceRepair.relationTypeHelp")}</span>
          </label>
          {relationSources.memories.length > 0 && <label style={labelStyle}>{t("sourceRepair.relationSource")}
            <select aria-label={t("sourceRepair.relationSource")} value={relationSource} onChange={(event) => setRelationSource(event.target.value)}>
              <option value="">{t("sourceRepair.relationNoSource")}</option>
              {relationSources.memories.map((memory) => <option key={memory.source_id} value={memory.source_id}>{memory.title || t("sourceRepair.untitledMemory")}</option>)}
            </select>
          </label>}
        </>}
      </fieldset>}

      {editable && checkId === CHECK_CLASSIFICATION && (
        <label style={labelStyle}>
          {t("sourceRepair.newMemoryType")}
          <select aria-label={t("sourceRepair.newMemoryType")} value={selectedType} onChange={(event) => setSelectedType(event.target.value as MemoryType | "")} disabled={busy}>
            <option value="">{t("sourceRepair.chooseMemoryType")}</option>
            {MEMORY_TYPES.map((type) => <option key={type} value={type}>{t(`importView.typeLabels.${type}`)}</option>)}
          </select>
        </label>
      )}
      {editable && checkId === CHECK_DUPLICATE_TITLES && (
        <label style={labelStyle}>
          {t("sourceRepair.newPageTitle")}
          <input aria-label={t("sourceRepair.newPageTitle")} value={newTitle} onChange={(event) => setNewTitle(event.target.value)} disabled={busy} />
        </label>
      )}
      {editable && checkId === CHECK_ENRICHMENT && (
        <fieldset className="source-repair-entities" disabled={busy}>
          <legend>{t("sourceRepair.existingEntities")}</legend>
          <p style={{ margin: 0, color: "var(--mem-text-secondary)" }}>{t("sourceRepair.sameScopeEntities", { scope: currentSpace ?? uncategorizedLabel })}</p>
          {entities.length === 0 ? <p style={{ margin: 0 }}>{t("sourceRepair.noEntities")}</p> : entities.map((entity) => (
            <label key={entity.id} className="source-repair-entity">
              <input
                type="checkbox"
                checked={selectedEntities.has(entity.id)}
                onChange={(event) => setSelectedEntities((old) => {
                  const next = new Set(old);
                  if (event.target.checked) next.add(entity.id); else next.delete(entity.id);
                  return next;
                })}
              />
              <span>{entity.name} <span style={{ color: "var(--mem-text-secondary)" }}>({t(`atlas.entityType.${entity.entity_type}`, { defaultValue: entity.entity_type })})</span></span>
            </label>
          ))}
          <p style={{ margin: 0, color: "var(--mem-text-secondary)" }}>{t("sourceRepair.emptySelectionAllowed")}</p>
        </fieldset>
      )}

      {manifest && (flow === "prepared" || flow === "applying" || flow === "uncertain_apply" || flow === "verifying" || flow === "applied_unverified" || flow === "restoring_service" || flow === "verified") && (
        <div className="source-repair-preview">
          <p style={{ margin: "0 0 8px", fontWeight: 600 }}>{t("sourceRepair.previewTitle")}</p>
          {mutationPreview?.kind === "reclassify_memory" && <p style={{ margin: 0 }}>{t("sourceRepair.typePreview", { before: isMemoryType(mutationPreview.before_memory_type) ? t(`importView.typeLabels.${mutationPreview.before_memory_type}`) : t("sourceRepair.unclassified"), after: t(`importView.typeLabels.${mutationPreview.after_memory_type}`) })}</p>}
          {mutationPreview?.kind === "rename_page_title" && <p style={{ margin: 0 }}>{t("sourceRepair.titlePreview", { before: mutationPreview.before_title, after: mutationPreview.after_title })}</p>}
          {mutationPreview?.kind === "complete_entity_extraction" && <p style={{ margin: 0 }}>{previewEntitiesMissing ? t("sourceRepair.selectedEntitiesUnavailable") : previewEntities.length > 0 ? t("sourceRepair.entityPreview", { entities: previewEntities.join(", ") }) : t("sourceRepair.entityPreviewEmpty")}</p>}
          {relationTarget && relationChange && <>
            {relationPreviewMissing && <p role="alert">{t("sourceRepair.relationPreviewMissing")}</p>}
            <p>{t(relationChange.kind === "add" ? "sourceRepair.relationAddPreview" : "sourceRepair.relationRetirePreview", {
              from: relationName(relationTarget.from_entity) ?? t("sourceRepair.relationUnavailable"),
              to: relationName(relationTarget.to_entity) ?? t("sourceRepair.relationUnavailable"),
              type: relationChange.kind === "add" ? relationChange.canonical_relation_type : relationSources?.relations.find((edge) => edge.id === relationTarget.relation_id)?.relation_type ?? t("sourceRepair.relationUnavailable"),
            })}</p>
            {relationChange.kind === "add" && <>
              {relationChange.requested_relation_type !== relationChange.canonical_relation_type && <p>{t("sourceRepair.relationNormalized", { requested: relationChange.requested_relation_type, canonical: relationChange.canonical_relation_type })}</p>}
              {relationChange.vocabulary_promotion && <p>{t("sourceRepair.relationPromotion", { type: relationChange.vocabulary_promotion })}</p>}
              {relationChange.retire_relation_ids.length > 0 && <>
                <p>{t("sourceRepair.relationReplaces")}</p>
                <ul>{relationChange.retire_relation_ids.map((id) => <li key={id}>{relationLabel(id) ?? t("sourceRepair.relationUnavailable")}</li>)}</ul>
              </>}
              <p>{relationChange.source_memory_id
                ? t("sourceRepair.relationSourcePreview", { source: relationSources?.memories.find((memory) => memory.source_id === relationChange.source_memory_id)?.title || t("sourceRepair.untitledMemory") })
                : t("sourceRepair.relationNoSource")}</p>
            </>}
          </>}
        </div>
      )}

      {actionHost ? createPortal(actions, actionHost) : actions}
      {flow === "preparing" && <p role="status" style={{ margin: 0 }}>{t("sourceRepair.preparing")}</p>}
      {flow === "applying" && <p role="status" style={{ margin: 0 }}>{t("sourceRepair.applying")}</p>}
      {flow === "verifying" && <p role="status" style={{ margin: 0 }}>{t("sourceRepair.verifying")}</p>}
      {flow === "restoring_service" && <p role="status" style={{ margin: 0 }}>{t("sourceRepair.restoringService")}</p>}
      {diagnosticDetails}
    </div>
  );
}
