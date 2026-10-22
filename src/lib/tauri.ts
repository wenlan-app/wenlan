// SPDX-License-Identifier: AGPL-3.0-only
import { invoke } from "@tauri-apps/api/core";
import type {
  ApplyRepairRequest, PrepareRepairRequest, PrepareCurrentRepairRequest, RepairApplyReceipt, RepairLintQuery,
  RepairLintReport, RepairManifest, RepairOperationStatus, RepairPrepareOperationRequest, RepairPrepareOperationStatus, RepairPlanEntriesPage, RepairPlanEntriesRequest, RepairRecovery,
  RepairPlanRequest, RepairPlanSummary, RepairRuntimeResumeApproval, RepairVerificationReceipt, VerifyRepairRequest,
} from "./repairTypes";
export { daemonMeetsFloor } from "./daemonVersion";

export async function repairLint(query: RepairLintQuery): Promise<RepairLintReport> {
  return invoke("repair_lint", { query });
}

export async function repairPrepare(request: PrepareRepairRequest): Promise<RepairManifest> {
  return invoke("repair_prepare", { request });
}

export async function repairPrepareCurrent(request: PrepareCurrentRepairRequest): Promise<RepairManifest> {
  return invoke("repair_prepare_current", { request });
}

export async function repairPrepareOperation(request: RepairPrepareOperationRequest): Promise<RepairPrepareOperationStatus> {
  return invoke("repair_prepare_operation", { request });
}

export async function repairPrepareOperationStatus(request: RepairPrepareOperationRequest): Promise<RepairPrepareOperationStatus> {
  return invoke("repair_prepare_operation_status", { request });
}

export async function repairPrepareOperationCancel(request: RepairPrepareOperationRequest): Promise<RepairPrepareOperationStatus> {
  return invoke("repair_prepare_operation_cancel", { request });
}

export async function repairRecovery(reviewId: string): Promise<RepairRecovery | null> {
  return invoke("repair_recovery", { reviewId });
}

export async function repairValidateManifest(manifest: RepairManifest): Promise<boolean> {
  return invoke("repair_validate_manifest", { manifest });
}

export async function repairApply(request: ApplyRepairRequest): Promise<RepairApplyReceipt> {
  return invoke("repair_apply", { request });
}

/** Observe an exact manifest without retrying its write. */
export async function repairOperationStatus(request: ApplyRepairRequest): Promise<RepairOperationStatus> {
  return invoke("repair_operation_status", { request });
}

/** A cancellation is confirmed only by the returned cancelled state. */
export async function repairCancel(request: ApplyRepairRequest): Promise<RepairOperationStatus> {
  return invoke("repair_cancel", { request });
}

export async function repairVerify(request: VerifyRepairRequest): Promise<RepairVerificationReceipt> {
  return invoke("repair_verify", { request });
}

/**
 * Ask native code to resume normal runtime service after a terminal
 * verification receipt. Native measures process identity and only restarts
 * positively owned services; it resolves once lifecycle handoff has
 * finished. Callers must still confirm normal service (e.g. `getActivity`)
 * before treating the repair as usable — a resolved promise here is not
 * itself proof the daemon answers requests again.
 */
export async function repairResumeRuntime(request: RepairRuntimeResumeApproval): Promise<void> {
  return invoke("repair_resume_runtime", { request });
}

export async function repairPlan(request: RepairPlanRequest): Promise<RepairPlanSummary> {
  return invoke("repair_plan", { request });
}

export async function repairPlanEntries(request: RepairPlanEntriesRequest): Promise<RepairPlanEntriesPage> {
  return invoke("repair_plan_entries", { request });
}

export async function setTrafficLightsVisible(visible: boolean): Promise<void> {
  return invoke("set_traffic_lights_visible", { visible });
}

export interface SearchResult {
  id: string;
  content: string;
  source: string;
  source_id: string;
  title: string;
  url: string | null;
  chunk_index: number;
  last_modified: number;
  score: number;
  chunk_type?: string;
  language?: string;
  semantic_unit?: string;
  memory_type?: string;
  domain?: string;
  space?: string | null;
  source_agent?: string;
  confidence?: number;
  confirmed?: boolean;
  entity_id?: string;
  entity_name?: string;
  quality?: string;
  is_archived?: boolean;
  is_recap?: boolean;
  structured_fields?: string | null;  // JSON string from Rust
  retrieval_cue?: string | null;
}

export interface IndexStatus {
  is_running: boolean;
  files_indexed: number;
  files_total: number;
  last_error: string | null;
  sources_connected: string[];
  reranker: RerankerStatus;
  reranker_light: RerankerStatus;
  reranker_mode: string;
}

export type RerankerStatus =
  | { state: "disabled" }
  | { state: "active"; model_id: string }
  | { state: "failed"; reason: string };

export interface SourceStatus {
  name: string;
  connected: boolean;
  requires_auth: boolean;
  last_sync: number | null;
  document_count: number;
  error: string | null;
}

type DomainCompat = { domain?: string | null; space?: string | null };

function withDomain<T extends DomainCompat>(item: T): T {
  if (item.domain !== undefined || item.space === undefined) return item;
  return { ...item, domain: item.space };
}

function withDomainArray<T extends DomainCompat>(items: T[]): T[] {
  return items.map(withDomain);
}

export async function search(
  query: string,
  limit?: number,
  sourceFilter?: string,
): Promise<SearchResult[]> {
  const results = await invoke<SearchResult[]>("search", {
    query,
    limit: limit ?? 10,
    sourceFilter: sourceFilter ?? null,
  });
  return withDomainArray(results);
}

export async function getIndexStatus(): Promise<IndexStatus> {
  return invoke("get_index_status");
}

export async function addWatchPath(path: string): Promise<void> {
  return invoke("add_watch_path", { path });
}

export async function removeWatchPath(path: string): Promise<void> {
  return invoke("remove_watch_path", { path });
}

export async function reindex(): Promise<void> {
  return invoke("reindex");
}

export async function listWatchPaths(): Promise<string[]> {
  return invoke("list_watch_paths");
}

export async function connectSource(sourceName: string): Promise<void> {
  return invoke("connect_source", { sourceName });
}

export async function disconnectSource(sourceName: string): Promise<void> {
  return invoke("disconnect_source", { sourceName });
}

export async function syncSource(sourceName: string): Promise<void> {
  return invoke("sync_source", { sourceName });
}

export async function listSources(): Promise<SourceStatus[]> {
  return invoke("list_sources");
}

// ===== Registered Sources =====

export type SourceTypeStr = "obsidian" | "logseq" | "directory" | "okf";
export type SyncStatusStr =
  | "Active"
  | "Paused"
  | { Error: string }
  | { Unavailable: string };

/**
 * Categorized sync error detail string. The Rust side sends this as
 * `Option<String>`, so any string is possible on the wire. The frontend
 * treats these as opaque keys and matches known values for targeted
 * messaging, falling through to a generic message otherwise.
 *
 * Known values (as of this writing): "google_drive_offline", "file_read_errors",
 * "document_enrichment_failed" (the files read fine, the worker gave up on them).
 */
export type SyncErrorDetail = string;

export interface RegisteredSource {
  id: string;
  source_type: SourceTypeStr;
  path: string;
  status: SyncStatusStr;
  last_sync: number | null;
  file_count: number;
  memory_count: number;
  last_sync_errors?: number;
  last_sync_error_detail?: SyncErrorDetail | null;
  /** Space new documents land in; set only for okf sources. */
  space?: string | null;
  /** okf sources: queued files not yet prepared after the last sync. */
  queued_files?: number;
  /** okf sources: files not yet handed to the queue after the last sync. */
  waiting_files?: number;
}

export interface SyncStats {
  files_found: number;
  ingested: number;
  skipped: number;
  errors: number;
  error_detail?: SyncErrorDetail | null;
  /** okf sources only: queued files not yet prepared after this sync. */
  queued_files?: number;
  /** okf sources only: files not yet handed to the queue after this sync. */
  waiting_files?: number;
}

export async function listRegisteredSources(): Promise<RegisteredSource[]> {
  return invoke("list_registered_sources");
}

/** `followChanges: false` registers an import-once source: read now and on
 *  Refresh, never by background checks. Omitted means follow changes. */
export async function addSource(
  sourceType: SourceTypeStr,
  path: string,
  followChanges?: boolean,
): Promise<RegisteredSource> {
  return invoke("add_source", followChanges === undefined ? { sourceType, path } : { sourceType, path, followChanges });
}

/** Switch a source between following changes and import-once. */
export async function setSourceFollowChanges(id: string, followChanges: boolean): Promise<RegisteredSource> {
  return invoke("set_source_follow_changes", { id, followChanges });
}

/** Import-once sources are stored as `Paused`, which background checks skip. */
export function sourceFollowsChanges(source: Pick<RegisteredSource, "status">): boolean {
  return source.status !== "Paused";
}

export async function removeSource(id: string): Promise<void> {
  return invoke("remove_source", { id });
}

export async function syncRegisteredSource(id: string): Promise<SyncStats> {
  return invoke("sync_registered_source", { id });
}

/** Read the running daemon's version string from GET /api/health. */
export async function getDaemonVersion(): Promise<string> {
  return invoke("daemon_version");
}

/** Read-only daemon/app version report for the version banner. Never restarts. */
export interface DaemonVersionStatus {
  daemon: string;
  app: string;
  matched: boolean;
  owner: "launchd" | "sidecar" | "unknown";
  program?: string | null;
}

/** Outcome of one self-heal attempt, emitted as `daemon://version`. */
export interface DaemonVersionEvent {
  daemon: string;
  app: string;
  matched: boolean;
  restarted: boolean;
  owner: "launchd" | "sidecar" | "unknown";
  error?: string | null;
  program?: string | null;
}

export async function getDaemonVersionStatus(): Promise<DaemonVersionStatus> {
  return invoke("daemon_version_status");
}

/**
 * Restart a mismatched daemon with the startup self-heal branch logic.
 * Resolves with the new health version; rejects with a typed error string
 * for the banner to show inline.
 */
export async function restartDaemon(): Promise<string> {
  return invoke("restart_daemon");
}

/** Stage a loose file into the managed dir and ensure it is a daemon source. */
export async function uploadSourceFile(path: string): Promise<RegisteredSource> {
  return invoke("upload_source_file", { path });
}

export interface IndexedFileInfo {
  source: string;
  source_id: string;
  title: string;
  url?: string | null;
  summary?: string;
  chunk_count: number;
  last_modified: number;
  processing?: boolean;
  memory_type?: string | null;
  domain?: string | null;
  space?: string | null;
  source_agent?: string | null;
  confidence?: number | null;
  confirmed?: boolean | null;
  pinned?: boolean;
}

export async function listIndexedFiles(space?: string): Promise<IndexedFileInfo[]> {
  const files = space === undefined
    ? await invoke<IndexedFileInfo[]>("list_indexed_files")
    : await invoke<IndexedFileInfo[]>("list_indexed_files", { space });
  return withDomainArray(files);
}

export interface SourceAnnotationPage {
  id: string;
  title: string;
}

export interface SourceAnnotation {
  source: string;
  source_id: string;
  notes: string;
  page_ids: string[];
  version: number;
  updated_at: number;
  pages: SourceAnnotationPage[];
}

export interface UpdateSourceAnnotationRequest {
  source: string;
  source_id: string;
  space?: string;
  expected_version: number;
  notes?: string;
  page_ids?: string[];
}

export async function getSourceAnnotation(
  source: string,
  sourceId: string,
  space?: string,
): Promise<SourceAnnotation> {
  return invoke<SourceAnnotation>("get_source_annotation", {
    source,
    sourceId,
    ...(space === undefined ? {} : { space }),
  });
}

export async function updateSourceAnnotation(
  req: UpdateSourceAnnotationRequest,
): Promise<SourceAnnotation> {
  return invoke<SourceAnnotation>("update_source_annotation", { req });
}

export async function deleteFileChunks(source: string, sourceId: string): Promise<void> {
  return invoke("delete_file_chunks", { source, sourceId });
}

export async function deleteByTimeRange(start: number, end: number): Promise<void> {
  return invoke("delete_by_time_range", { start, end });
}

export async function deleteBulk(items: { source: string; sourceId: string }[]): Promise<void> {
  return invoke("delete_bulk", {
    items: items.map((i) => ({ source: i.source, source_id: i.sourceId })),
  });
}

export async function openFile(url: string): Promise<void> {
  const path = url.startsWith("file://") ? url.slice(7) : url;
  return invoke("open_file", { path });
}

/**
 * Open a search result's target. Unlike {@link openFile}, this does NOT strip
 * a `file://` prefix — `open_search_result` refuses `file:` URLs on purpose,
 * since the value came from a search result and is attacker-influenced (see
 * the Rust command's doc comment). Local files reach it as bare paths, which
 * it checks against an allowlist of indexed document types.
 */
export async function openSearchResult(url: string): Promise<void> {
  return invoke("open_search_result", { url });
}

/** AI tool families with a link that opens a new chat with the question typed
 *  in (never sent). Mirrors `prompt_link` in `app/src/ask_tool.rs`. */
export const PROMPT_LINK_FAMILIES: readonly string[] = ["claude-desktop", "claude-code", "codex", "cursor"];

/** Open a connected AI tool with `prompt` in its composer. Rejects when the
 *  tool has no such link or nothing on this machine opens it. */
export async function openToolWithPrompt(family: string, prompt: string): Promise<void> {
  return invoke("open_tool_with_prompt", { family, prompt });
}

export interface SourceDirEntry {
  name: string;
  isDirectory: boolean;
}

/**
 * List the immediate entries of a directory (for the Sources browser).
 * Reads via the Rust `read_source_dir` command rather than the webview fs
 * plugin, because `fs:default` has no path scope — a registered source's path
 * isn't readable from the webview on a fresh launch. Names only, dotfiles
 * hidden. Sorting is the caller's job.
 */
export async function readSourceDir(path: string): Promise<SourceDirEntry[]> {
  return invoke("read_source_dir", { path });
}

export interface ObsidianVault {
  name: string;
  path: string;
}

/**
 * Read the user's real Obsidian vaults from Obsidian's own vault registry
 * (macOS: `~/Library/Application Support/obsidian/obsidian.json`), most
 * recently opened first. Read in Rust because the webview's `fs:default`
 * scope only covers app-specific directories — it can't reach Obsidian's
 * registry. Never throws: no Obsidian, an unreadable registry, or a vault
 * whose folder is gone all just mean an empty list.
 */
export async function detectObsidianVaults(): Promise<ObsidianVault[]> {
  return invoke("detect_obsidian_vaults");
}

/**
 * Read a text file's full contents for inline preview in the detail pane.
 * Backed by the Rust `read_text_file` command (same reason as readSourceDir:
 * the webview fs plugin is unscoped). Caller must gate to text extensions —
 * the command rejects files over 512 KiB. Named `readSourceText` to avoid
 * colliding with the fs plugin's own `readTextFile`.
 */
export async function readSourceText(path: string): Promise<string> {
  return invoke("read_text_file", { path });
}

export interface ChunkDetail {
  id: string;
  content: string;
  chunk_index: number;
  chunk_type: string | null;
  language: string | null;
}

export async function getChunks(source: string, sourceId: string, space?: string): Promise<ChunkDetail[]> {
  return invoke("get_chunks", space === undefined ? { source, sourceId } : { source, sourceId, space });
}

export async function updateChunk(id: string, content: string): Promise<void> {
  return invoke("update_chunk", { id, content });
}

export interface QuickCaptureRequest {
  title?: string;
  content: string;
  tags?: string[];
  domain?: string;
}

export async function quickCapture(req: QuickCaptureRequest): Promise<number> {
  return invoke("quick_capture", { req });
}

export async function clipboardWrite(text: string): Promise<void> {
  const { writeText } = await import("tauri-plugin-clipboard-x-api");
  await writeText(text);
}

export interface IngestWebpageRequest {
  url: string;
  title: string;
  content: string;
  metadata?: Record<string, string> | null;
  create_only?: boolean;
}

export interface IngestResponse {
  chunks_created: number;
  document_id: string;
}

export async function ingestWebpage(req: IngestWebpageRequest): Promise<IngestResponse> {
  return invoke("ingest_webpage", { req });
}

export async function getApiKey(): Promise<string | null> {
  return invoke("get_api_key");
}

export async function setApiKey(key: string): Promise<void> {
  return invoke("set_api_key", { key });
}

export async function getModelChoice(): Promise<[string | null, string | null]> {
  return invoke("get_model_choice");
}

// The daemon config endpoint is patch-based: null preserves the current value.
export async function setModelChoice(
  routineModel: string | null,
  synthesisModel: string | null
): Promise<void> {
  return invoke("set_model_choice", { routineModel, synthesisModel });
}

// ── Resolved routing (daemon ≥ PR #357) ──────────────────────────────────
// Feature detection: getResolvedRouting() resolves to null on a daemon that
// predates the endpoint (the live 0.13.2 daemon) — callers render LEGACY mode.
// Never contains key material.

/** How one job resolves. `mode`: "pinned" | "pinned_unavailable" | "auto".
 *  `source` (everyday): "anthropic"|"external"|"on_device"|"basic";
 *  (synthesis): "anthropic"|"external"|"on_device"|"none". `pin`: the raw
 *  configured source pin, or `null` when unpinned — distinct from `source`
 *  (the RESOLVED source): on a `pinned_unavailable` result the two differ,
 *  letting the app say "Pinned to X — using Y". */
export interface JobRoute {
  source: string;
  model: string | null;
  mode: string;
  pin: string | null;
}

export interface ResolvedRouting {
  everyday: JobRoute;
  synthesis: JobRoute;
  pool: {
    anthropic: { configured: boolean; everyday_model: string | null; synthesis_model: string | null };
    external: { endpoint: string; model: string } | null;
    on_device: { selected: string | null; loaded: boolean } | null;
  };
}

/** Resolved per-job routing, or null when the daemon lacks the endpoint (404).
 *  A null return is the signal to render source pins read-only (LEGACY mode). */
export async function getResolvedRouting(): Promise<ResolvedRouting | null> {
  return invoke("get_resolved_routing");
}

// Patch-based like setModelChoice: null leaves a pin untouched, "" clears it, a
// source name pins. Only call once getResolvedRouting() returned non-null.
//
// onlyIfUnset makes a fill-the-blanks write atomic on the daemon: a named pin
// lands only on a job that holds none. Pass it whenever the pins being sent came
// from an earlier routing read, so a pin the user chose since that read wins.
// Leave it off for a choice the user just made, because that one must overwrite.
//
// The key is omitted when false, so every call that predates the flag puts the
// same arguments on the wire as before. This mirrors the Rust side, which adds
// only_if_unset to the daemon request body only when it is true.
export async function setSourcePin(
  everydaySource: string | null,
  synthesisSource: string | null,
  onlyIfUnset = false
): Promise<void> {
  const args: Record<string, unknown> = { everydaySource, synthesisSource };
  if (onlyIfUnset) args.onlyIfUnset = true;
  return invoke("set_source_pin", args);
}

/** Explicit automatic-AI preference. An older daemon without the field keeps
 * its legacy behavior; the daemon distinguishes that from a new Off install. */
export async function getBackgroundAiEnabled(): Promise<boolean> {
  return invoke("get_background_ai_enabled");
}

export async function setBackgroundAiEnabled(enabled: boolean): Promise<void> {
  return invoke("set_background_ai_enabled", { enabled });
}

export interface SystemInfo {
  total_ram_gb: number;
  available_ram_gb: number;
  has_metal: boolean;
  has_cuda: boolean;
  os: string;
  arch: string;
  recommended_builtin: string;
}

export async function getSystemInfo(): Promise<SystemInfo> {
  return invoke("get_system_info");
}

export async function getExternalLlm(): Promise<[string | null, string | null]> {
  return invoke("get_external_llm");
}

// The daemon config endpoint is patch-based: null endpoint/model preserves the
// current value. apiKey tri-state (spec §7.2): undefined/null = preserve stored
// key, "" = clear, non-empty = replace. Only pass a key when
// useDaemonVersion().supportsExternalKey — 0.12 daemons must never see it.
export async function setExternalLlm(
  endpoint: string | null,
  model: string | null,
  apiKey?: string | null
): Promise<void> {
  return invoke("set_external_llm", { endpoint, model, apiKey: apiKey ?? null });
}

export interface TestLlmResponse {
  response: string;
}

export async function testExternalLlm(
  endpoint: string,
  model: string,
  apiKey?: string | null
): Promise<TestLlmResponse> {
  return invoke("test_external_llm", { endpoint, model, apiKey: apiKey ?? null });
}

/** True when the daemon (≥ 0.13) has a stored external-provider API key.
 *  Always false on 0.12. The key value itself is never readable. */
export async function getExternalLlmKeyConfigured(): Promise<boolean> {
  return invoke("get_external_llm_key_configured");
}

/** Model auto-discovery: GET {endpoint}/models (OpenAI `{data:[{id}]}` shape),
 *  Bearer auth when a key is given, 5s timeout. Spec §1. */
export async function listExternalModels(
  endpoint: string,
  apiKey?: string | null
): Promise<string[]> {
  return invoke("list_external_models", { endpoint, apiKey: apiKey ?? null });
}

export interface OnDeviceModelEntry {
  id: string;
  display_name: string;
  param_count: string;
  ram_required_gb: number;
  file_size_gb: number;
  cached: boolean;
}

export interface OnDeviceModelResponse {
  /// ID of the model currently loaded in the daemon (if any).
  loaded: string | null;
  /// ID the user has selected in config (may differ from loaded).
  selected: string | null;
  /// All available models with per-model cache state.
  models: OnDeviceModelEntry[];
}

export async function getOnDeviceModel(): Promise<OnDeviceModelResponse> {
  return invoke("get_on_device_model");
}

/// Triggers download + hot-load. Long-running (minutes for 2.7GB).
export async function downloadOnDeviceModel(modelId: string): Promise<void> {
  return invoke("download_on_device_model", { modelId });
}

/** Bytes downloaded so far for an in-flight on-device model download, read
 *  from the hf-hub cache's in-progress `.part` file. `null` when no download
 *  is in progress. There is no reliable total to compare this against — see
 *  callers for how the estimated file size is used only as a rough, always
 *  under-run, denominator. */
export async function onDeviceModelDownloadBytes(): Promise<number | null> {
  return invoke("on_device_model_download_bytes");
}

export interface ActivitySummary {
  id: string;
  started_at: number;
  ended_at: number;
  is_live: boolean;
  app_names: string[];
}

export async function listActivities(): Promise<ActivitySummary[]> {
  return invoke("list_activities");
}

export async function rebuildActivities(): Promise<number> {
  return invoke("rebuild_activities");
}

export type CaptureStats = Record<string, number>;

export async function getCaptureStats(): Promise<CaptureStats> {
  return invoke("get_capture_stats");
}

// ── Activity status (daemon ≥ activity route; GET /api/activity) ───────────
// Field names and enum string values mirror wenlan_types::activity exactly.

// "unknown" is a word from a daemon newer than this app: the app's Rust client
// reads any unrecognized value in these enums as it rather than failing the
// read. activitySentence.ts narrows each one to the words this build knows.
export type ActivityState =
  | "up_to_date"
  | "off"
  | "organizing"
  | "waiting_for_idle"
  | "blocked"
  | "unknown";
export type ActivityAssetKind = "memories" | "entities" | "pages" | "unknown";
export type ActivityStepName =
  | "store"
  | "summarize"
  | "link"
  | "detect"
  | "confirm"
  | "write"
  | "unknown";
export type ActivityStepState = "idle" | "running" | "blocked" | "unknown";
export type ActivityJob = "everyday" | "synthesis" | "unknown";
export type ActivityLane =
  | "on_device"
  | "external"
  | "anthropic"
  | "basic"
  | "none"
  | "unknown";

export interface ActivityRoute {
  job: ActivityJob;
  lane: ActivityLane;
  model: string | null;
  mode: string;
  available: boolean;
}

export interface ActivityStep {
  name: ActivityStepName;
  state: ActivityStepState;
  done: number;
  total: number;
  failed: number;
  job: ActivityJob | null;
}

export interface ActivityAssetStatus {
  kind: ActivityAssetKind;
  state: ActivityStepState;
  done: number;
  total: number;
  blocked: number;
  steps: ActivityStep[];
}

/** Open refinement rows for one action and status, verbatim labels. */
export interface ActivityRefinementGroup {
  action: string;
  status: string;
  count: number;
}

/** Open refinement suggestions. Reported only: never part of `state`. */
export interface ActivityRefinement {
  ready_for_review: number;
  not_ready: number;
  groups: ActivityRefinementGroup[];
}

export interface ActivityResponse {
  state: ActivityState;
  last_activity_at: number | null;
  assets: ActivityAssetStatus[];
  everyday: ActivityRoute;
  synthesis: ActivityRoute;
  refinement: ActivityRefinement;
}

/** `/api/activity` as sent: a daemon from before `refinement` omits it. */
type ActivityWire = Omit<ActivityResponse, "refinement"> & {
  refinement?: ActivityRefinement;
};

export async function getActivity(): Promise<ActivityResponse> {
  // A flat generic keeps `get_activity` visible to the command scanners in
  // reviewCommandContract.test.ts and preview/mocks/live-invoke.test.ts.
  const response = await invoke<ActivityWire>("get_activity");
  // The browser preview proxies the daemon's raw JSON, so a daemon from
  // before `refinement` existed sends none. The app's Rust client already
  // fills zeros through its serde default.
  return {
    ...response,
    refinement: response.refinement ?? { ready_for_review: 0, not_ready: 0, groups: [] },
  };
}

// ── Tags ────────────────────────────────────────────────────────────────

export interface TagData {
  tags: string[];
  document_tags: Record<string, string[]>;
  categories: string[];
  document_categories: Record<string, string>;
}

// ── Spaces ──────────────────────────────────────────────────────────────

export interface Space {
  id: string;
  name: string;
  description: string | null;
  suggested: boolean;
  starred: boolean;
  sort_order: number;
  memory_count: number;
  entity_count: number;
  created_at: number;
  updated_at: number;
}

export async function listAllTags(): Promise<TagData> {
  return invoke("list_all_tags");
}

export async function setDocumentTags(
  source: string,
  sourceId: string,
  tags: string[],
): Promise<string[]> {
  return invoke("set_document_tags", { source, sourceId, tags });
}

export async function deleteTag(name: string): Promise<void> {
  return invoke("delete_tag", { name });
}

export async function suggestTags(
  source: string,
  sourceId: string,
  lastModified: number,
): Promise<string[]> {
  return invoke("suggest_tags", { source, sourceId, lastModified });
}

export async function listSpaces(): Promise<Space[]> {
  return invoke("list_spaces");
}

export async function getSpace(name: string): Promise<Space | null> {
  return invoke("get_space", { name });
}

export async function createSpace(
  name: string,
  description?: string,
): Promise<Space> {
  return invoke("create_space", { name, description: description ?? null });
}

export async function updateSpace(
  name: string,
  newName: string,
  description?: string,
): Promise<Space> {
  return invoke("update_space", { name, newName, description: description ?? null });
}

export async function deleteSpace(name: string): Promise<void> {
  return invoke("delete_space", { name });
}

export interface MoveSpaceResponse {
  affected: number;
}

export async function moveSpace(from: string, to: string): Promise<MoveSpaceResponse> {
  return invoke("move_space", { from, to });
}

export async function confirmSpace(name: string): Promise<void> {
  return invoke("confirm_space", { name });
}

export async function reorderSpace(name: string, newOrder: number): Promise<void> {
  return invoke("reorder_space", { name, newOrder });
}

export async function toggleSpaceStarred(name: string): Promise<boolean> {
  return invoke("toggle_space_starred", { name });
}

export async function setDocumentSpace(
  source: string,
  sourceId: string,
  spaceId: string,
): Promise<void> {
  return invoke("set_document_space", { source, sourceId, spaceId });
}

// ── Session Snapshots ───────────────────────────────────────────────────

export interface SessionSnapshot {
  id: string;
  activity_id: string;
  started_at: number;
  ended_at: number;
  primary_apps: string[];
  summary: string;
  tags: string[];
  capture_count: number;
}

export async function getSessionSnapshots(limit?: number): Promise<SessionSnapshot[]> {
  return invoke("get_session_snapshots", { limit: limit ?? 10 });
}

export interface SnapshotCapture {
  source_id: string;
  app_name: string;
  window_title: string;
  timestamp: number;
  source: string;
}

export async function getSnapshotCaptures(snapshotId: string): Promise<SnapshotCapture[]> {
  return invoke("get_snapshot_captures", { snapshotId });
}

export interface SnapshotCaptureWithContent {
  source_id: string;
  app_name: string;
  window_title: string;
  timestamp: number;
  source: string;
  content: string;
  summary: string | null;
}

export async function getSnapshotCapturesWithContent(snapshotId: string): Promise<SnapshotCaptureWithContent[]> {
  return invoke("get_snapshot_captures_with_content", { snapshotId });
}

export async function deleteSnapshot(snapshotId: string): Promise<void> {
  return invoke("delete_snapshot", { snapshotId });
}

// ── Memory Facets ───────────────────────────────────────────────────

export type MemoryType = 'identity' | 'preference' | 'decision' | 'lesson' | 'gotcha' | 'fact';

export interface MemoryFacet {
  type: MemoryType;
  label: string;
  color: string;
  description: string;
}

export const MEMORY_FACETS: MemoryFacet[] = [
  { type: 'identity', label: 'Identity', color: 'indigo', description: 'Who you are, always injected into context' },
  { type: 'preference', label: 'Preference', color: 'orange', description: 'How you like things done, always injected' },
  { type: 'decision', label: 'Decision', color: 'amber', description: 'Choices made with rationale and tradeoffs' },
  { type: 'lesson', label: 'Lesson', color: 'emerald', description: 'Positive learnings to reuse later' },
  { type: 'gotcha', label: 'Gotcha', color: 'red', description: 'Traps, failures, and warnings to avoid' },
  { type: 'fact', label: 'Fact', color: 'zinc', description: 'Durable knowledge, standard hybrid search' },
];

// Canonical memory-type colors aligned with Wenlan's --mem-accent-* palette:
//   identity  -> indigo  (--mem-accent-indigo: depth, foundational)
//   preference -> orange (--mem-accent-warm: personal, warm)
//   decision  -> amber   (--mem-accent-amber: weight, decisive)
//   lesson    -> emerald (--mem-accent-sage: reusable learning)
//   gotcha    -> red     (warning, avoid)
//   fact      -> zinc    (neutral, informational)
//   goal      -> emerald (legacy rows only)
export const FACET_COLORS: Record<string, string> = {
  identity: "bg-indigo-500/20 text-indigo-700 dark:text-indigo-400 border-indigo-500/30",
  preference: "bg-orange-500/20 text-orange-700 dark:text-orange-400 border-orange-500/30",
  decision: "bg-amber-500/20 text-amber-700 dark:text-amber-400 border-amber-500/30",
  lesson: "bg-emerald-500/20 text-emerald-700 dark:text-emerald-400 border-emerald-500/30",
  gotcha: "bg-red-500/20 text-red-700 dark:text-red-400 border-red-500/30",
  fact: "bg-zinc-500/20 text-zinc-700 dark:text-zinc-400 border-zinc-500/30",
  goal: "bg-emerald-500/20 text-emerald-700 dark:text-emerald-400 border-emerald-500/30",
};

// One agent-slug prettifier for every surface (list row, card, detail).
export const AGENT_DISPLAY: Record<string, string> = {
  "claude-code": "Claude Code",
  "claude-desktop": "Claude Desktop",
  claude: "Claude",
  chatgpt: "ChatGPT",
  "chatgpt-mcp": "ChatGPT",
  codex: "Codex",
  cursor: "Cursor",
  "gemini-cli": "Gemini CLI",
  "muse-code": "Muse Code",
  windsurf: "Windsurf",
  zed: "Zed",
  wenlan: "Wenlan",
  // The desktop app's own writer id (`callerId` on its saves and rebuilds).
  "wenlan-app": "Wenlan",
};

export function agentDisplayName(slug: string | null | undefined): string | null {
  if (!slug) return null;
  return AGENT_DISPLAY[slug.trim().toLowerCase()] ?? slug;
}

export type StabilityTier = "protected" | "standard" | "ephemeral";

export const STABILITY_TIERS: Record<string, StabilityTier> = {
  identity: "protected",
  preference: "protected",
  fact: "standard",
  decision: "standard",
  lesson: "standard",
  gotcha: "standard",
  goal: "protected",
};

export interface PendingRevision {
  source_id: string;
  content: string;
  source_agent: string | null;
}

/**
 * What a staged revision proposes to rewrite. The daemon resolves this; never
 * infer it from an id prefix or from `source_agent`.
 *
 * `target_source_id` is a `memories.source_id` for `"memory"` and a `pages.id`
 * for `"page"`. Both are accepted and dismissed through the same two verbs, but
 * the "before" side of the diff comes from a different read
 * (`getMemoryDetail` vs `getPage`).
 */
export type RevisionTargetKind = "memory" | "page";

export interface PendingRevisionItem {
  target_source_id: string;
  revision_source_id: string;
  revision_content: string;
  source_agent: string | null;
  last_modified: number;
  target_kind: RevisionTargetKind;
  /** Why a page revision card needs review: "locked", "background" or
   * "owned". Absent when the daemon cannot say (memory revisions, old cards). */
  review_reason?: string | null;
}

export interface RevisionAcceptResponse {
  target_source_id: string;
  revision_source_id: string;
  wrote: boolean;
}

export interface RevisionDismissResponse {
  target_source_id: string;
  /** False: rejecting a card writes nothing to the page or memory. */
  wrote: boolean;
  dismissed?: boolean;
}

export interface ContradictionDismissResponse {
  source_id: string;
  wrote: boolean;
}

export interface MemoryVersionItem {
  source_id: string;
  title: string;
  content: string;
  memory_type: string | null;
  confirmed: boolean;
  supersedes: string | null;
  last_modified: number;
}

export interface MemoryRevisionEntry {
  source_id: string;
  depth: number;
  title: string;
  content_preview: string;
  last_modified: number;
  source_agent?: string | null;
  supersede_mode?: string | null;
  delta_summary?: string | null;
}

export interface ListMemoryRevisionsResponse {
  current_source_id: string;
  chain_depth: number;
  entries: MemoryRevisionEntry[];
}

export interface PageChangelogEntry {
  version: number;
  at: number;
  edited_by: string;
  delta_summary?: string | null;
  incoming_source_ids?: string[] | null;
  citations_summary?: string | null;
  /** The agent behind an `agent_refresh` write (the `x-agent-name` header). */
  agent?: string | null;
  /** Numeric twin of `delta_summary`; absent on entries written before them. */
  char_delta?: number | null;
  sources_added?: number | null;
  sources_removed?: number | null;
}

export interface ListPageRevisionsResponse {
  page_id: string;
  current_version: number;
  user_edited: boolean;
  stale_reason?: string | null;
  entries: PageChangelogEntry[];
}

// ── Memory Page ─────────────────────────────────────────────────────

/** Lifecycle state of an entity: `detected` entities are the automatic index
 * the system grows and prunes itself; `established` entities have earned
 * substance and appear in the Wiki; `archived` entities stay resolvable so a
 * recurring mention does not mint a duplicate. */
export type EntityStatus = "detected" | "established" | "archived";

export interface Entity {
  id: string;
  name: string;
  entity_type: string;
  domain: string | null;
  space?: string | null;
  source_agent: string | null;
  confidence: number | null;
  confirmed: boolean;
  created_at: number;
  updated_at: number;
  /** Number of memories linked to this entity. Zero for a freshly detected entity. */
  memory_count: number;
  status: EntityStatus;
  /** How the entity became established: "manual", "auto:memories",
   * "auto:citation", or null while detected or archived-before-established. */
  established_by: string | null;
}

/** `POST /api/memory/entities` (legacy list) and `POST /api/memory/entities/query`
 * filter. Every field is optional; `limit`/`offset` are ignored by the legacy route. */
export interface ListEntitiesRequest {
  entity_type?: string;
  space?: string;
  status?: EntityStatus;
  /** Inclusive lower bound on linked-memory count. */
  min_memories?: number;
  /** Inclusive upper bound on linked-memory count. */
  max_memories?: number;
  /** Case-insensitive substring match on name and aliases. */
  query?: string;
  /** Page size for `/query`; default 100, clamped to 1000. */
  limit?: number;
  offset?: number;
}

export interface ListEntitiesResponse {
  entities: Entity[];
  total: number;
}

/** Which entities a bulk archive/restore acts on: explicit `ids`, or every
 * entity matching `filter` (whose `limit`/`offset` are ignored). Exactly one
 * must be present. */
export interface EntitySelection {
  ids?: string[];
  filter?: ListEntitiesRequest;
}

export interface ArchiveEntitiesRequest extends EntitySelection {
  dry_run: boolean;
}

export interface RestoreEntitiesRequest extends EntitySelection {
  dry_run: boolean;
}

export interface EntityBulkResponse {
  count: number;
  entity_ids: string[];
  dry_run: boolean;
}

export interface Observation {
  id: string;
  entity_id: string;
  content: string;
  source_agent: string | null;
  confidence: number | null;
  confirmed: boolean;
  created_at: number;
}

export interface RelationWithEntity {
  id: string;
  relation_type: string;
  direction: string;
  entity_id: string;
  entity_name: string;
  entity_type: string;
  source_agent: string | null;
  created_at: number;
}

export interface EntityDetail {
  entity: Entity;
  observations: Observation[];
  relations: RelationWithEntity[];
}

export interface EntitySearchResult {
  entity: Entity;
  distance: number;
}

/** A relation edge from the bulk graph read: both endpoints by id. */
export interface GraphRelation {
  id: string;
  from_entity: string;
  to_entity: string;
  relation_type: string;
  source_agent: string | null;
  created_at: number;
}

/** A memory drawn as a graph node. */
export interface GraphMemoryNode {
  source_id: string;
  title: string;
  memory_type: string | null;
  space: string | null;
  confirmed: boolean;
  last_modified: number;
}

/** A memory-to-entity link, drawn as an edge. */
export interface GraphMemoryLink {
  memory_id: string;
  entity_id: string;
}

/** A wiki page drawn as a graph node. Entity shadow pages are not wiki pages
 *  and never appear here — the entity itself is already the node. The daemon
 *  sends no community for a page (neither store can answer it honestly); the
 *  map derives one by inheritance, see cartography.ts. */
export interface GraphPageNode {
  id: string;
  title: string;
  space: string | null;
  creation_kind: string;
  entity_id: string | null;
  /** RFC 3339, as `pages.last_modified` stores it. */
  last_modified: string;
}

/** One endpoint of a page link: which collection to look the id up in. */
export interface GraphRef {
  kind: "page" | "entity" | "memory";
  id: string;
}

/** A typed edge with a page on at least one end. */
export interface GraphPageLink {
  from: GraphRef;
  to: GraphRef;
  link_type: "wikilink" | "about" | "cites";
}

/**
 * The whole knowledge graph for the current space scope in one read. Replaces
 * the per-entity detail fan-out the Graph view used to do, which could only
 * afford the first 20 entities.
 */
export interface KnowledgeGraph {
  entities: Entity[];
  relations: GraphRelation[];
  memories: GraphMemoryNode[];
  memory_links: GraphMemoryLink[];
  pages: GraphPageNode[];
  page_links: GraphPageLink[];
}

export interface MemoryItem {
  source_id: string;
  title: string;
  content: string;
  summary: string | null;
  source_text?: string | null;
  memory_type: string | null;
  domain: string | null;
  space?: string | null;
  source_agent: string | null;
  confidence: number | null;
  confirmed: boolean;
  stability?: "new" | "learned" | "confirmed";
  pinned: boolean;
  supersedes: string | null;
  last_modified: number;
  chunk_count: number;
  entity_id?: string | null;
  quality?: string | null;
  is_recap?: boolean;
  enrichment_status?: string;
  supersede_mode?: string;
  /**
   * True when an `'archive'`-mode superseder replaced this memory. The row stays
   * listed — `'archive'` keeps the predecessor visible but muted — and the UI
   * fades and labels it. Distinct from `supersede_mode`, which is this row's
   * behaviour toward the rows *it* supersedes.
   */
  is_archived?: boolean;
  pending_revision?: boolean;
  merged_from?: string[] | null;
  /** The AI conversation that wrote this memory (`ses_...`), when one did. */
  session_id?: string | null;
  structured_fields?: string | null;  // JSON string from Rust
  retrieval_cue?: string | null;
  access_count?: number;
  version?: number;
  changelog?: Array<{
    version: number;
    at: number;
    delta: string;
    source_agent?: string | null;
    incoming_source_id?: string | null;
  }>;
}

export interface DomainInfo {
  name: string;
  count: number;
}

export interface TypeBreakdown {
  memory_type: string;
  count: number;
}

export interface MemoryStats {
  total: number;
  new_today: number;
  confirmed: number;
  domains: DomainInfo[];
  by_type?: TypeBreakdown[];
  entity_linked?: number;
  enrichment_pending?: number;
}

// ── Pages ───────────────────────────────────────────────────────────

export interface PageCitation {
  occurrence: number;
  marker: number;
  source_kind: "memory" | "external_url" | "external_file" | "authored";
  locator: string;
  score: number;
  status: "verified" | "unverified";
  scope: "sentence" | "paragraph";
}

/**
 * Both M5 truth axes for one page. Only ever present when this `Page` came
 * back from an explicit-browse call (`listPagesExplicitBrowse`,
 * `getPage(id, "explicit")`) against a daemon
 * whose truth cutover has begun — see `useTruthStatus`. `supported` and
 * `human_reviewed` are independent axes; neither implies the other, and
 * neither is the same thing as `Page.review_status` above (a pre-existing,
 * separate distillation-faithfulness signal).
 */
export interface PageTruth {
  supported: boolean;
  /** Three-state support. Older daemons omit it; read it through `pageSupport`. */
  support?: "supported" | "unsupported" | "unevaluated" | "unknown";
  human_reviewed: boolean;
}

/** The support axis in three states. A daemon that predates `support`, or a
 * word this build does not know, reads as unevaluated: never as a verdict. */
export function pageSupport(truth: PageTruth): "supported" | "unsupported" | "unevaluated" {
  if (truth.support === "supported" || truth.support === "unsupported" || truth.support === "unevaluated") return truth.support;
  return truth.supported ? "supported" : "unevaluated";
}

/** Where the daemon stands on the M5 truth cutover. See `useTruthStatus`. */
export interface TruthStatus {
  cutover_generation: number;
  contract_version: number;
}

export interface Page {
  /** Live Markdown projection filename, relative to the daemon knowledge root.
   * null means no verified file; absent on older daemon versions. */
  storage_path?: string | null;
  /** Initial draft destination. Active pages are located by storage_path. */
  folder_path?: string | null;
  projection_status?: string | null;
  projection_error?: string | null;
  id: string;
  title: string;
  summary: string | null;
  content: string;
  entity_id: string | null;
  domain: string | null;
  space?: string | null;
  source_memory_ids: string[];
  version: number;
  status: string;
  /** How this Page entered the inventory. Missing on older daemon versions. */
  creation_kind?: string | null;
  /** Trust state for an already-persisted Page; distinct from page candidates. */
  review_status?: string | null;
  created_at: string;
  last_compiled: string;
  last_modified: string;
  // Staleness tracking (added in migration 40)
  sources_updated_count?: number;
  stale_reason?: string | null;
  /**
   * Why the last automatic re-synthesis of this stale page was discarded
   * (today: the citation verification gate). Set only while the page is
   * blocked; the daemon clears it on every successful write and on new
   * source changes. Missing on older daemon versions (migration 127).
   */
  refresh_blocked_reason?: string | null;
  user_edited?: boolean;
  /** Review lock: machine writes wait as revision cards. Omitted while false. */
  review_required?: boolean;
  /**
   * The notes this note relies on, in any state. Present on a page read, absent
   * when the note relies on nothing and on every inventory/list entry.
   */
  basis?: BasisRef[];
  /**
   * The lines of code this note relies on, in any state. Present on a page
   * read, absent when the note relies on no code and on every list entry.
   */
  code_basis?: CodeBasisRef[];
  citations?: PageCitation[];
  /**
   * Both M5 truth axes, present only when this page was fetched via an
   * explicit-browse call and the daemon's truth cutover is live. Absent
   * (not `null`) on every automatic/background read by daemon contract.
   */
  truth?: PageTruth | null;
}

export interface RenamePageResponse {
  id: string;
  title: string;
  version: number;
}

export async function renamePage(
  id: string,
  title: string,
  expectedVersion: number,
): Promise<RenamePageResponse> {
  return invoke("rename_page", { id, title, expectedVersion });
}

export interface KnowledgeFolder { path: string; parent_path: string; name: string; }
export interface KnowledgeFoldersResponse { folders: KnowledgeFolder[]; truncated: boolean; }
export function knowledgeFoldersList(): Promise<KnowledgeFoldersResponse> {
  return invoke("knowledge_folders_list");
}
export function knowledgeFolderCreate(parentPath: string, name: string): Promise<{ path: string }> {
  return invoke("knowledge_folder_create", { parentPath, name });
}
export function pageMove(id: string, expectedStoragePath: string, folderPath: string, operationId: string): Promise<{ storage_path: string }> {
  return invoke("page_move", { id, expectedStoragePath, folderPath, operationId });
}

export interface CreatePageInput {
  title: string;
  content: string;
  space: string | null;
}

export interface CreatePageResponse {
  id: string;
  attached_to?: string | null;
  warnings: string[];
}

export interface PageDraftSnapshotInput {
  title: string;
  content: string;
  space: string | null;
}

export interface PageDraftWriteInput extends PageDraftSnapshotInput {
  clientDraftId: string;
  folderPath?: string;
}

export interface UpdatePageDraftInput extends PageDraftSnapshotInput {
  id: string;
  expectedVersion: number;
}

export interface PageDraftVersionInput {
  id: string;
  expectedVersion: number;
}

export type PageDraftErrorCode =
  | "draft_version_conflict"
  | "page_title_conflict"
  | "page_draft_not_found"
  | "page_draft_id_conflict";

type PageDraftErrorPayload = {
  code: PageDraftErrorCode;
  error?: string;
  current_version?: number;
  existing_page_id?: string;
  existing_page_title?: string;
};

export class PageDraftApiError extends Error {
  readonly name = "PageDraftApiError";

  constructor(
    message: string,
    readonly code: PageDraftErrorCode,
    readonly currentVersion?: number,
    readonly existingPageId?: string,
    readonly existingPageTitle?: string,
  ) {
    super(message);
  }
}

function pageDraftErrorText(error: unknown): string {
  if (typeof error === "string") return error;
  if (
    typeof error === "object"
    && error !== null
    && typeof Reflect.get(error, "message") === "string"
  ) {
    return Reflect.get(error, "message") as string;
  }
  return String(error);
}

/**
 * The daemon's own sentence out of a rejected Tauri command.
 *
 * A rejected command carries a formatted string, not an Error:
 * `HTTP POST /api/... returned 409 Conflict: {"error":"..."}` (app/src/api.rs
 * `post_empty`). The URL and the status are ours; the part worth showing a
 * person is the `error` field of the JSON body the daemon sent, which is where
 * a route explains what it refused and what it did instead. Returns null when
 * the rejection carries no daemon body, so callers keep their own wording for
 * transport failures.
 */
export function daemonErrorMessage(error: unknown): string | null {
  const text = pageDraftErrorText(error);
  const objectStart = text.indexOf("{");
  if (objectStart < 0) return null;
  let payload: unknown;
  try {
    payload = JSON.parse(text.slice(objectStart));
  } catch {
    return null;
  }
  if (typeof payload !== "object" || payload === null) return null;
  const message = Reflect.get(payload, "error");
  return typeof message === "string" && message.trim() !== "" ? message : null;
}

/** True when the daemon refused a page revision card because the page moved
 * past the version the card was proposed on. The daemon has already discarded
 * the card; it can never apply. */
export function isPageRevisionOutdated(error: unknown): boolean {
  return daemonErrorMessage(error)?.startsWith("page_revision_outdated") ?? false;
}

/** True when the daemon refused a page revision card because a page the
 * change relied on (a basis) was archived, deleted or moved since. The daemon
 * has already discarded the card and left the page unchanged. */
export function isPageRevisionBasisRefused(error: unknown): boolean {
  return daemonErrorMessage(error)?.startsWith("page_revision_basis_refused") ?? false;
}

/** The daemon's guard when a page delete is refused because the row belongs
 * to an entity: `{"error": "This page belongs to the entity '<name>'. Archive
 * or delete it from Entities", "entity_id": "..."}`. Isolated here so the
 * payload shape can move without touching the callers that render it. Returns
 * null for any other rejection (transport failure, a plain-string error, or a
 * daemon body with no `entity_id`). */
export interface EntityGuardError {
  readonly message: string;
  readonly entityId: string;
}

export function parseEntityGuardError(error: unknown): EntityGuardError | null {
  const text = pageDraftErrorText(error);
  const objectStart = text.indexOf("{");
  if (objectStart < 0) return null;
  let payload: unknown;
  try {
    payload = JSON.parse(text.slice(objectStart));
  } catch {
    return null;
  }
  if (typeof payload !== "object" || payload === null) return null;
  const entityId = Reflect.get(payload, "entity_id");
  if (typeof entityId !== "string" || entityId.trim() === "") return null;
  const message = Reflect.get(payload, "error");
  return {
    message: typeof message === "string" && message.trim() !== "" ? message : text,
    entityId,
  };
}

function parsePageDraftError(error: unknown): PageDraftApiError | null {
  const message = pageDraftErrorText(error);
  const objectStart = message.indexOf("{");
  if (objectStart < 0) return null;

  let payload: PageDraftErrorPayload;
  try {
    payload = JSON.parse(message.slice(objectStart)) as PageDraftErrorPayload;
  } catch {
    return null;
  }
  if (
    payload.code !== "draft_version_conflict"
    && payload.code !== "page_title_conflict"
    && payload.code !== "page_draft_not_found"
    && payload.code !== "page_draft_id_conflict"
  ) {
    return null;
  }
  return new PageDraftApiError(
    payload.error ?? message,
    payload.code,
    payload.current_version,
    payload.existing_page_id,
    payload.existing_page_title,
  );
}

async function invokePageDraft<T>(
  command: string,
  args: Record<string, unknown>,
): Promise<T> {
  try {
    return await invoke<T>(command, args);
  } catch (error) {
    throw parsePageDraftError(error) ?? error;
  }
}

/** @deprecated Use {@link Page} instead. Kept for gradual migration. */
export type Concept = Page;

export interface PageSource {
  page_id: string;
  memory_source_id: string;
  linked_at: number;
  link_reason?: string | null;
}

/** @deprecated Use {@link PageSource} instead. Kept for gradual migration. */
export type ConceptSource = PageSource;

export interface PageSourceWithMemory {
  source: PageSource;
  memory: MemoryItem | null;
}

/** @deprecated Use {@link PageSourceWithMemory} instead. Kept for gradual migration. */
export type ConceptSourceWithMemory = PageSourceWithMemory;

export interface PageLinkOutbound {
  label: string;
  target_page_id: string | null;
  target_title?: string | null;
}

export interface PageLinkInbound {
  source_page_id: string;
  label: string;
}

export interface PageLinksResponse {
  outbound: PageLinkOutbound[];
  inbound: PageLinkInbound[];
}

export interface OrphanLink {
  label: string;
  count: number;
}

export interface OrphanLinksResponse {
  min_count: number;
  orphan_labels: OrphanLink[];
}

export interface DistillReviewResponse {
  pages_created: number;
  clusters_found?: number | null;
  hint?: string | null;
  scoped: boolean;
  created_ids: string[];
  pending: DistillPendingCluster[];
  stale_pages: DistillStalePage[];
  stale_truncated: boolean;
  orphan_topics: DistillOrphanTopic[];
}

export interface DistillPendingCluster {
  source_ids: string[];
  contents: string[];
  entity_id?: string | null;
  entity_name?: string | null;
  space?: string | null;
  estimated_tokens: number;
  centroid_embedding?: number[] | null;
  existing_page_id?: string | null;
  existing_page_title?: string | null;
  new_memory_count?: number | null;
}

export interface DistillStalePage {
  page_id: string;
  title: string;
  summary?: string | null;
  source_memory_ids: string[];
  sources_updated_count?: number | null;
  stale_reason?: string | null;
  user_edited?: boolean | null;
}

export interface DistillOrphanTopic {
  label: string;
  count: number;
}

export interface PageRedistillResponse {
  status: "ok" | "skipped" | string;
  updated: boolean;
  /**
   * "Update from sources" on a note that relies on other notes stages a preview
   * card instead of writing: `gated` with the card to review.
   */
  gated?: boolean;
  revision_card_id?: string | null;
  /** The note already says what its sources say; nothing was staged. */
  unchanged?: boolean;
  hint?: string | null;
  /**
   * Present when `updated === false` because the daemon discarded the rebuild
   * (today: the citation verification gate) rather than finding nothing to do.
   */
  reason?: string | null;
}

export async function getPageSources(
  pageId: string,
): Promise<PageSourceWithMemory[]> {
  const sources = await invoke<PageSourceWithMemory[]>("get_page_sources", { pageId });
  return sources.map((source) => ({
    ...source,
    memory: source.memory ? withDomain(source.memory) : null,
  }));
}

/** @deprecated Use {@link getPageSources} instead. */
export async function getConceptSources(
  conceptId: string,
): Promise<PageSourceWithMemory[]> {
  return getPageSources(conceptId);
}

export async function getPageLinks(pageId: string): Promise<PageLinksResponse> {
  return invoke("get_page_links", { pageId });
}

export interface PageReviewRequiredResponse {
  page_id: string;
  review_required: boolean;
}

export async function setPageReviewRequired(
  pageId: string,
  required: boolean,
): Promise<PageReviewRequiredResponse> {
  return invoke("set_page_review_required", { pageId, required });
}

// ── Note bases (a note that relies on other notes) ───────────────────

/** `ok` is current; the rest need a look or have nothing to compare with. */
export type BasisState =
  | "ok"
  | "pending"
  | "archived"
  | "deleted"
  | "unavailable";

export interface BasisRef {
  page_id: string;
  /** Live title; the last known one once deleted; empty when not visible. */
  title: string;
  state: BasisState;
  from_version: number;
  /** Absent once the basis is deleted. */
  to_version?: number | null;
  /** Every change since the last check was a small edit. */
  minor_only?: boolean;
}

export interface BasisDiffEdit {
  version: number;
  edited_by: string;
  /** Unix seconds. */
  at: number;
  minor: boolean;
  delta_summary: string;
  /** The agent behind an `agent_refresh` edit; absent for other writers. */
  agent?: string | null;
}

export interface BasisDiff {
  from_version: number;
  to_version: number;
  from_content: string;
  to_content: string;
  edits: BasisDiffEdit[];
}

export type BasisDiffResult =
  | { outcome: "available"; diff: BasisDiff }
  | { outcome: "unavailable"; reason: string; message: string };

export type BasisConfirmResult =
  | { outcome: "confirmed"; basis: BasisRef }
  | { outcome: "conflict"; message: string };

export interface BasisPendingCiter {
  page_id: string;
  title: string;
  minor_only: boolean;
}

export interface BasisPendingEntry {
  basis_page_id: string;
  title: string;
  changed_version?: number | null;
  state: BasisState;
  citers: BasisPendingCiter[];
}

/** Use `basisPageId` as a source of `pageId`, at the version it is now. */
export async function markPageBasis(
  pageId: string,
  basisPageId: string,
): Promise<BasisRef> {
  return invoke("mark_page_basis", { pageId, basisPageId });
}

export async function unmarkPageBasis(
  pageId: string,
  basisPageId: string,
): Promise<{ removed: boolean }> {
  return invoke("unmark_page_basis", { pageId, basisPageId });
}

/** "No impact": `version` is the one the person looked at. */
export async function confirmPageBasis(
  pageId: string,
  basisPageId: string,
  version: number,
): Promise<BasisConfirmResult> {
  return invoke("confirm_page_basis", { pageId, basisPageId, version });
}

export async function getPageBasisDiff(
  pageId: string,
  basisPageId: string,
): Promise<BasisDiffResult> {
  return invoke("get_page_basis_diff", { pageId, basisPageId });
}

/** Every changed source with the notes waiting on it; `space` narrows it. */
export async function listBasisPending(
  space: string | null = null,
): Promise<BasisPendingEntry[]> {
  return invoke("list_basis_pending", { space });
}

// ── Code bases (a note that relies on lines of code) ─────────────────

/**
 * `ok`: the lines read as they did. `moved`: the same lines at other numbers
 * (the span here is already the new one). `pending`: the lines changed.
 * `unavailable`: the file or repository is gone (see `reason`).
 */
export type CodeBasisState = "ok" | "moved" | "pending" | "unavailable";

export type CodeBasisUnavailableReason =
  | "file_missing"
  | "repo_missing"
  | "outside_repo"
  | "unreadable"
  | "too_large";

export interface CodeBasisRef {
  /** Absolute. */
  repo_root: string;
  /** Relative to `repo_root`, `/` separated. */
  path: string;
  /** 1-based, inclusive. */
  line_start: number;
  line_end: number;
  state: CodeBasisState;
  /** Unix seconds when the lines were first seen changed; only while pending. */
  changed_at?: number | null;
  /** Why the code is unavailable. */
  reason?: CodeBasisUnavailableReason | null;
  /** What the background check concluded about a pending row: `"stale"` or `"needs_human"`. */
  ai_verdict?: string;
  /** One sentence from that check, in the note's language. */
  explanation?: string;
}

/** Which row of a note is meant: the span the row has now. */
export interface CodeBasisLocator {
  repo_root: string;
  path: string;
  line_start: number;
  line_end: number;
}

/** The locator of a row, with exactly the four fields the daemon accepts. */
export function codeBasisLocator(row: CodeBasisLocator): CodeBasisLocator {
  return {
    repo_root: row.repo_root,
    path: row.path,
    line_start: row.line_start,
    line_end: row.line_end,
  };
}

export type CodeBasisConfirmResult =
  | { outcome: "confirmed"; code_basis: CodeBasisRef }
  | { outcome: "conflict"; message: string };

export interface CodeBasisPendingEntry {
  page_id: string;
  title: string;
  /** The note's pending and unavailable code bases. */
  pending: CodeBasisRef[];
}

export interface CodeBasisCited {
  /**
   * The git blob id (40 hex characters) of the whole file as it was when the
   * note last cited or confirmed the lines; null for rows written before the
   * id was kept.
   */
  cited_blob: string | null;
  /** Unix seconds. */
  confirmed_at: number;
  state: string;
}

/** Notes whose code changed or went missing; `space` narrows it. */
export async function listCodeBasisPending(
  space: string | null = null,
): Promise<CodeBasisPendingEntry[]> {
  return invoke("list_code_basis_pending", { space });
}

/** "No impact": the lines as they read now are accounted for. */
export async function confirmPageCodeBasis(
  pageId: string,
  locator: CodeBasisLocator,
): Promise<CodeBasisConfirmResult> {
  return invoke("confirm_page_code_basis", { pageId, locator: codeBasisLocator(locator) });
}

export async function removePageCodeBasis(
  pageId: string,
  locator: CodeBasisLocator,
): Promise<{ removed: boolean }> {
  return invoke("remove_page_code_basis", { pageId, locator: codeBasisLocator(locator) });
}

export async function getPageCodeBasisCited(
  pageId: string,
  locator: CodeBasisLocator,
): Promise<CodeBasisCited> {
  return invoke("get_page_code_basis_cited", { pageId, locator: codeBasisLocator(locator) });
}

/**
 * The requested lines (1-based, inclusive) of a code file as it reads now.
 * Read in Rust like `readSourceText`: the webview cannot reach a repository.
 * Rejects when the file cannot be read.
 */
export async function readCodeLines(
  path: string,
  lineStart: number,
  lineEnd: number,
): Promise<string[]> {
  return invoke("read_code_lines", { path, lineStart, lineEnd });
}

/**
 * The requested lines (1-based, inclusive) of a file as git stored it: the
 * blob `blob` of the repository at `repoRoot`. Rejects when git is missing,
 * the object is gone or the repository moved.
 */
export async function readGitBlobLines(
  repoRoot: string,
  blob: string,
  lineStart: number,
  lineEnd: number,
): Promise<string[]> {
  return invoke("read_git_blob_lines", { repoRoot, blob, lineStart, lineEnd });
}

export async function listOrphanLinks(minCount?: number): Promise<OrphanLinksResponse> {
  return invoke("list_orphan_links", { minCount: minCount ?? null });
}

export async function distillReview(): Promise<DistillReviewResponse> {
  return invoke("distill_review");
}

export async function redistillPage(pageId: string): Promise<PageRedistillResponse> {
  return invoke("redistill_page", { pageId });
}

// ── Profiles & Agent Connections ─────────────────────────────────────

export interface Profile {
  id: string;
  name: string;
  display_name: string | null;
  email: string | null;
  bio: string | null;
  avatar_path: string | null;
  created_at: number;
  updated_at: number;
}

export interface AgentConnection {
  id: string;
  /** Canonical technical ID (lowercase, hyphen-case). Matches `x-agent-name`. */
  name: string;
  /** Human-readable name shown in UI. Falls back to KNOWN_CLIENT_DISPLAY_NAMES → name. */
  display_name: string | null;
  agent_type: string;
  description: string | null;
  enabled: boolean;
  trust_level: string;
  last_seen_at: number | null;
  memory_count: number;
  created_at: number;
  updated_at: number;
}

// Knowledge Graph

export async function createEntity(
  name: string,
  entityType: string,
  domain?: string,
): Promise<string> {
  return invoke("create_entity_cmd", {
    name,
    entityType,
    domain: domain ?? null,
  });
}

export async function listEntities(
  entityType?: string,
  domain?: string,
): Promise<Entity[]> {
  const entities = await invoke<Entity[]>("list_entities_cmd", {
    entityType: entityType ?? null,
    domain: domain ?? null,
  });
  return withDomainArray(entities);
}

export async function searchEntities(
  query: string,
  limit?: number,
): Promise<EntitySearchResult[]> {
  const results = await invoke<EntitySearchResult[]>("search_entities_cmd", {
    query,
    limit: limit ?? null,
  });
  return results.map((result) => ({
    ...result,
    entity: withDomain(result.entity),
  }));
}

export async function getKnowledgeGraph(): Promise<KnowledgeGraph> {
  const graph = await invoke<KnowledgeGraph>("get_knowledge_graph_cmd");
  return { ...graph, entities: withDomainArray(graph.entities) };
}

export async function getEntityDetail(entityId: string): Promise<EntityDetail> {
  const detail = await invoke<EntityDetail>("get_entity_detail_cmd", { entityId });
  return { ...detail, entity: withDomain(detail.entity) };
}

export async function updateObservation(observationId: string, content: string): Promise<void> {
  return invoke("update_observation_cmd", { observationId, content });
}

export async function deleteObservation(observationId: string): Promise<void> {
  return invoke("delete_observation_cmd", { observationId });
}

export async function addObservation(
  entityId: string,
  content: string,
  sourceAgent?: string,
  confidence?: number,
): Promise<string> {
  return invoke("add_observation_cmd", {
    entityId,
    content,
    sourceAgent: sourceAgent ?? null,
    confidence: confidence ?? null,
  });
}

export async function deleteEntity(entityId: string): Promise<void> {
  return invoke("delete_entity_cmd", { entityId });
}

export async function confirmEntity(entityId: string, confirmed: boolean): Promise<void> {
  return invoke("confirm_entity_cmd", { entityId, confirmed });
}

/** Changes a topic's canonical entity type and refreshes its contextual identity. */
export async function changeEntityType(entityId: string, entityType: string): Promise<void> {
  return invoke("change_entity_type_cmd", { entityId, entityType });
}

/** Filtered, paged entity list backing the Entities view (tabs, search, type
 * and memory-count chips). */
export async function queryEntities(
  filter: ListEntitiesRequest,
): Promise<ListEntitiesResponse> {
  const result = await invoke<ListEntitiesResponse>("query_entities_cmd", { filter });
  return { ...result, entities: withDomainArray(result.entities) };
}

/** Archives every entity in `req.ids`, or every entity matching `req.filter`
 * when `ids` is absent. `req.dry_run` previews the count without mutating. */
export async function archiveEntities(
  req: ArchiveEntitiesRequest,
): Promise<EntityBulkResponse> {
  return invoke("archive_entities_cmd", { req });
}

/** Exact inverse of {@link archiveEntities} for the selected archived entities. */
export async function restoreEntities(
  req: RestoreEntitiesRequest,
): Promise<EntityBulkResponse> {
  return invoke("restore_entities_cmd", { req });
}

export async function confirmObservation(observationId: string, confirmed: boolean): Promise<void> {
  return invoke("confirm_observation_cmd", { observationId, confirmed });
}

// Store Memory

export interface StoreMemoryRequest {
  content: string;
  memory_type?: string;
  domain?: string;
  source_agent?: string;
  title?: string;
  tags?: string[];
  confidence?: number;
  supersedes?: string;
  structured_fields?: Record<string, string>;
  retrieval_cue?: string;
}

export interface StoreMemoryResponse {
  source_id: string;
  warnings?: string[];
  /**
   * Background-enrichment state. `"pending"` when the daemon will enrich
   * asynchronously, `"paused"` when a selected model cannot run, and `"off"`
   * when background AI organization is disabled.
   */
  enrichment?: "pending" | "not_needed" | string;
  /**
   * Prose nudge the caller agent can relay verbatim. Empty when enrichment
   * is not_needed.
   */
  hint?: string;
}

export async function storeMemory(req: StoreMemoryRequest): Promise<StoreMemoryResponse> {
  return invoke("store_memory", { req });
}

// Memories

export async function listMemoriesRich(
  domain?: string,
  memoryType?: string,
  confirmed?: boolean,
  limit?: number,
): Promise<MemoryItem[]> {
  const memories = await invoke<MemoryItem[]>("list_memories_cmd", {
    domain: domain ?? null,
    memoryType: memoryType ?? null,
    confirmed: confirmed ?? null,
    limit: limit ?? null,
  });
  return withDomainArray(memories);
}

export async function getMemoryDetail(sourceId: string): Promise<MemoryItem | null> {
  const memory = await invoke<MemoryItem | null>("get_memory_detail", { sourceId });
  return memory ? withDomain(memory) : null;
}

export interface EnrichmentStepStatus {
  step: string;
  status: string;
  error?: string | null;
  attempts: number;
}

export interface EnrichmentStatusResponse {
  source_id: string;
  summary: string;
  steps: EnrichmentStepStatus[];
}

export async function getEnrichmentStatus(
  sourceId: string,
): Promise<EnrichmentStatusResponse> {
  return invoke("get_enrichment_status", { sourceId });
}

/** Batch-fetch multiple memories by source_id in one round trip. Missing ids are silently omitted. */
export async function listMemoriesByIds(ids: string[]): Promise<MemoryItem[]> {
  if (ids.length === 0) return [];
  const memories = await invoke<MemoryItem[]>("list_memories_by_ids", { ids });
  return withDomainArray(memories);
}

export async function getMemoryStats(): Promise<MemoryStats> {
  return invoke("get_memory_stats_cmd");
}

export interface TopMemory {
  source_id: string;
  content: string;
  memory_type: string | null;
  domain: string | null;
  times_retrieved: number;
}

export interface HomeStats {
  total: number;
  new_today: number;
  confirmed: number;
  total_ingested: number;
  active_insights: number;
  distilled_today: number;
  distilled_all: number;
  sources_archived: number;
  times_served_today: number;
  words_saved_today: number;
  times_served_week: number;
  words_saved_week: number;
  times_served_all: number;
  words_saved_all: number;
  corrections_active: number;
  top_memories: TopMemory[];
}

export async function getHomeStats(): Promise<HomeStats> {
  return invoke("get_home_stats");
}

export async function updateMemory(
  sourceId: string,
  content?: string,
  domain?: string,
  confirmed?: boolean,
  memoryType?: MemoryType,
  expectedContent?: string,
): Promise<void> {
  return invoke("update_memory_cmd", {
    sourceId,
    content: content ?? null,
    domain: domain ?? null,
    confirmed: confirmed ?? null,
    memoryType: memoryType ?? null,
    ...(expectedContent === undefined ? {} : { expectedContent }),
  });
}

export async function reclassifyMemory(
  sourceId: string,
  memoryType: MemoryType,
): Promise<string> {
  return invoke("reclassify_memory_cmd", { sourceId, memoryType });
}

export async function getVersionChain(
  sourceId: string,
): Promise<MemoryVersionItem[]> {
  return invoke("get_version_chain_cmd", { sourceId });
}

/** One AI conversation a write came from. Times are Unix seconds. */
export interface AgentSession {
  id: string;
  tool: string;
  external_id: string;
  title?: string | null;
  cwd?: string | null;
  repo_root?: string | null;
  repo_remote?: string | null;
  repo_branch?: string | null;
  repo_commit?: string | null;
  /** Where the tool keeps the transcript. Never shown; only its presence matters. */
  transcript_path?: string | null;
  space?: string | null;
  first_write_at: number;
  last_write_at: number;
}

export interface SessionMemoryEntry {
  source_id: string;
  title: string;
  memory_type?: string | null;
  space?: string | null;
  created_at: number;
}

export interface SessionPageEntry {
  id: string;
  title: string;
  version: number;
  edited_at: number;
}

export interface SessionHandoffEntry {
  space: string;
  brief_version: number;
  summary: string;
  created_at: number;
}

/** `recall`: a memory search (label is the query). `brief`: a Space Brief read (label is the Space). `read_page`: a note read in full (label is the note id). */
export type SessionReadKind = "recall" | "brief" | "read_page";

export interface SessionReadEntry {
  kind: SessionReadKind;
  label: string;
  /** The note's title, for `read_page` only. */
  title?: string | null;
  created_at: number;
}

/** A note that cites a memory the conversation wrote, without the conversation having written the note. */
export interface SessionCitedByEntry {
  id: string;
  title: string;
  cited_at: number;
}

export interface SessionDetail {
  session: AgentSession;
  memories: SessionMemoryEntry[];
  pages: SessionPageEntry[];
  handoffs: SessionHandoffEntry[];
  /** What the conversation looked up, newest first (at most 10). Absent from older daemons. */
  reads?: SessionReadEntry[];
  /** How many lookups there are in all, which can exceed `reads.length`. */
  reads_total?: number;
  /** Notes that later used a memory this conversation wrote. */
  cited_by?: SessionCitedByEntry[];
  /** The transcript file named by `session.transcript_path` is still on this machine. */
  transcript_available?: boolean;
}

/** `wrote`: the conversation wrote a version of the note. `source`: it wrote a memory the note cites. */
export type PageSessionVia = "wrote" | "source";

export interface PageSessionEntry {
  session: AgentSession;
  via: PageSessionVia;
}

/** The conversation and what it left behind; null when it is not visible here. */
export async function getSessionDetail(sessionId: string): Promise<SessionDetail | null> {
  return invoke("get_session_detail", { sessionId });
}

export async function getPageSessions(pageId: string): Promise<PageSessionEntry[]> {
  const response = await invoke<{ sessions?: PageSessionEntry[] }>("get_page_sessions", { pageId });
  return response?.sessions ?? [];
}

export async function getMemoryRevisions(
  sourceId: string,
): Promise<ListMemoryRevisionsResponse> {
  return invoke("get_memory_revisions", { sourceId });
}

export async function getPageRevisions(
  pageId: string,
): Promise<ListPageRevisionsResponse> {
  return invoke("get_page_revisions", { pageId });
}

export async function listPendingRevisions(limit?: number): Promise<PendingRevisionItem[]> {
  return invoke("list_pending_revisions", { limit: limit ?? null });
}

export async function acceptPendingRevision(sourceId: string): Promise<RevisionAcceptResponse> {
  return invoke("accept_pending_revision", { sourceId });
}

export async function dismissPendingRevision(sourceId: string): Promise<RevisionDismissResponse> {
  return invoke("dismiss_pending_revision", { sourceId });
}

export async function dismissContradiction(sourceId: string): Promise<ContradictionDismissResponse> {
  return invoke("dismiss_contradiction", { sourceId });
}

export async function confirmMemory(sourceId: string, confirmed: boolean = true): Promise<void> {
  return invoke("confirm_memory", { sourceId, confirmed });
}

/** Permanently deletes the memory — the "forget" half of the curate flow. */
export async function deleteMemory(sourceId: string): Promise<void> {
  return invoke("delete_memory", { sourceId });
}

export async function getPendingRevision(sourceId: string): Promise<PendingRevision | null> {
  return invoke("get_pending_revision", { sourceId });
}

// ===== Refinery Queue =====

export type ProposalAction =
  | "entity_merge"
  | "relation_conflict"
  | "detect_contradiction"
  | "suggest_entity"
  | "dedup_merge"
  | "page_merge"
  | "cross_space_discovery"
  | "page_keep_or_archive"
  | "lint_repair_review"
  | "vocab_promote"
  | "unknown";

export type RefinementPayload =
  | {
      action: "entity_merge";
      existing_id: string;
      new_id: string;
      similarity: number;
    }
  | {
      action: "relation_conflict";
      existing_id: string;
      new_id: string;
      from: string;
      to: string;
      old_type: string;
      new_type: string;
    }
  | { action: "detect_contradiction" }
  | { action: "suggest_entity"; name_hint?: string | null }
  | { action: "dedup_merge" }
  | {
      action: "page_merge";
      left_page_id: string;
      right_page_id: string;
      source_overlap: number;
      source_overlap_ratio: number;
    }
  | {
      action: "cross_space_discovery";
      memory_count: number;
      spaces: string[];
    }
  | {
      action: "page_keep_or_archive";
      page_id: string;
      source_count: number;
      allowed_actions?: Array<"accept" | "dismiss" | "pick_space">;
    }
  | {
      action: "lint_repair_review";
      check_id: string;
      occurrence_digest: string;
      owner_binding_digest: string;
      issue: string;
      choices: string[];
      suggested_research_queries: string[];
    }
  | { action: "vocab_promote"; kind: string; old_value: string; category?: string | null };

export interface RefinementProposalSummary {
  id: string;
  action: ProposalAction;
  source_ids: string[];
  payload?: RefinementPayload | null;
  confidence: number;
  created_at: string;
}

export interface ListRefinementsResponse {
  proposals: RefinementProposalSummary[];
}

export interface AcceptRefinementResponse {
  id: string;
  action_applied: string;
}

export interface RejectRefinementResponse {
  id: string;
}

export async function listRefinements(limit?: number): Promise<ListRefinementsResponse> {
  return invoke("list_refinements", { limit: limit ?? null });
}

export async function acceptRefinement(id: string): Promise<AcceptRefinementResponse> {
  return invoke("accept_refinement", { id });
}

export async function rejectRefinement(id: string): Promise<RejectRefinementResponse> {
  return invoke("reject_refinement", { id });
}

// ===== Pages =====

/**
 * `intent: "explicit"` marks this as a human-initiated wiki browse, so a
 * daemon with a live truth cutover attaches `Page.truth`
 * (`crates/wenlan-core/src/truth_manifest.rs`: `GET /api/pages/{id}` is a
 * NamedPage route). Pass it only for a page a person navigated to — a
 * background poll or an agent-driven read must stay `"automatic"` (the
 * default), since the daemon durably records every explicit call.
 */
export async function getPage(
  id: string,
  intent: "automatic" | "explicit" = "automatic",
): Promise<Page | null> {
  const command = intent === "explicit" ? "get_page_explicit_browse" : "get_page";
  const page = await invoke<Page | null>(command, { id });
  return page ? withDomain(page) : null;
}

/** Explicit-browse sibling of {@link getPage}, for call sites that always browse. */
export async function getPageExplicitBrowse(id: string): Promise<Page | null> {
  return getPage(id, "explicit");
}

/**
 * Where the daemon stands on the M5 truth cutover. `null` on a daemon that
 * predates the field or has not begun the cutover — read both as "not live"
 * (`crates/wenlan-types/src/responses.rs::TruthStatus::cutover_live`). This
 * is itself an automatic read: cutover state is not page prose, so it
 * carries no explicit-browse marker. See `useTruthStatus`.
 */
export async function getTruthStatus(): Promise<TruthStatus | null> {
  return invoke<TruthStatus | null>("get_truth_status");
}

/**
 * What the daemon answered when asked to mark a page human-reviewed.
 *
 * Every variant but `applied` is a refusal the user is told something
 * different about, which is why this is a tagged union rather than a boolean
 * plus an error string. Binding spec:
 * `docs/plans/2026-07-27-m5-presence-threat-model.md` in the daemon repo.
 */
export type PageReviewOutcome =
  /** The mark landed. A retry of one that already succeeded arrives here too. */
  | { kind: "applied"; reviewed_page_version: number }
  /** The page changed after it was read, so the approval is about text nobody
   *  is looking at any more. Reload and ask again. */
  | { kind: "stale" }
  /** The capability's sixty seconds ran out in flight. Clicking again mints a
   *  fresh one, so this one is worth retrying as-is. */
  | { kind: "expired" }
  /** The capability did not verify, or named a page the daemon does not have. */
  | { kind: "invalid" }
  /** Its one-shot nonce was already spent. */
  | { kind: "replayed" }
  /** The daemon cannot check presence at all — the install secret is missing
   *  or unreadable. Not a licence to proceed. */
  | { kind: "unavailable" }
  /** This daemon has no review route; it predates the endpoint. */
  | { kind: "unsupported" };

/**
 * Marks a page human-reviewed.
 *
 * `content` is the body the caller actually rendered. It is not decoration:
 * the Tauri backend hashes it and binds the presence capability to that exact
 * text, so passing anything other than what the human read either fails as
 * `stale` or, worse, attributes their approval to content they never saw.
 *
 * The capability itself is minted and submitted entirely inside the Rust
 * backend and never crosses this boundary in either direction (threat model
 * T1) — which is why this takes a page and its content, and not a token.
 */
export async function reviewPage(id: string, content: string): Promise<PageReviewOutcome> {
  return invoke<PageReviewOutcome>("review_page", { pageId: id, content });
}

/** Whether a review can succeed, and when it cannot, which fact stops it. */
export type PageReviewAvailability =
  /** Both the daemon and this build can carry it through. */
  | "ready"
  /** No review route on this daemon — too old, or unreachable. Fixable by
   *  upgrading, so the copy points there. */
  | "daemon_unsupported"
  /** This build cannot mint a capability on any daemon (owner-only secret
   *  protection is Unix-only today), so upgrading the daemon would not help
   *  and the copy must not suggest it. */
  | "platform_unsupported";

export async function pageReviewSupported(): Promise<PageReviewAvailability> {
  return invoke<PageReviewAvailability>("page_review_supported");
}

export async function createPage(input: CreatePageInput): Promise<CreatePageResponse> {
  const normalizedSpace = input.space?.trim() || null;
  const result = await invoke<Omit<CreatePageResponse, "warnings"> & { warnings?: string[] }>("create_page", {
    title: input.title.trim(),
    content: input.content.trim(),
    space: normalizedSpace,
  });
  return { ...result, warnings: result.warnings ?? [] };
}

export async function createPageDraft(input: PageDraftWriteInput): Promise<Page> {
  const page = await invokePageDraft<Page>("create_page_draft", {
    clientDraftId: input.clientDraftId,
    ...(input.folderPath !== undefined ? { folderPath: input.folderPath } : {}),
    title: input.title,
    content: input.content,
    space: input.space?.trim() || null,
  });
  return withDomain(page);
}

export async function updatePageDraft(input: UpdatePageDraftInput): Promise<Page> {
  const page = await invokePageDraft<Page>("update_page_draft", {
    id: input.id,
    expectedVersion: input.expectedVersion,
    title: input.title,
    content: input.content,
    space: input.space?.trim() || null,
  });
  return withDomain(page);
}

export async function publishPageDraft(input: PageDraftVersionInput): Promise<Page> {
  const page = await invokePageDraft<Page>("publish_page_draft", {
    id: input.id,
    expectedVersion: input.expectedVersion,
  });
  return withDomain(page);
}

export async function discardPageDraft(input: PageDraftVersionInput): Promise<void> {
  await invokePageDraft<void>("discard_page_draft", {
    id: input.id,
    expectedVersion: input.expectedVersion,
  });
}

/** @deprecated Use {@link getPage} instead. */
export async function getConcept(id: string): Promise<Page | null> {
  return getPage(id);
}

export interface UpdatePageInput {
  id: string;
  content: string;
  expectedVersion: number;
  callerId: "wenlan-app";
  operationId: string;
  /** Also confirm one basis of this note, if it is still at `version`. */
  confirmBasis?: { basis_page_id: string; version: number };
}

export type UpdatePageFailureKind =
  | "not_found"
  | "auth_required"
  | "payload_too_large"
  | "validation"
  | "rate_limited"
  | "server"
  | "other";

export type BasisConfirmOutcome = "confirmed" | "moved_on" | "not_saved";

export type UpdatePageOutcome =
  | { outcome: "saved"; basisConfirm?: BasisConfirmOutcome }
  | {
      outcome: "upgrade_required";
      reportedVersion: string;
      requiredFloor: string;
    }
  | { outcome: "conflict"; message: string }
  | {
      outcome: "failure";
      kind: UpdatePageFailureKind;
      status: number;
      message: string;
    };

export async function updatePage(
  input: UpdatePageInput,
): Promise<UpdatePageOutcome> {
  return invoke("update_page", { ...input });
}

/** @deprecated Use {@link updatePage} instead. */
export async function updateConcept(
  input: UpdatePageInput,
): Promise<UpdatePageOutcome> {
  return updatePage(input);
}

export type PageEditorDiagnosticInput =
  | {
      event: "daemon_floor_blocked";
      reportedVersion: string | null;
      requiredFloor: string;
    }
  | {
      event: "editor_fallback";
      reason: "load" | "construction";
    };

export async function recordPageEditorDiagnostic(
  input: PageEditorDiagnosticInput,
): Promise<void> {
  return invoke("record_page_editor_diagnostic", { ...input });
}

export async function deletePage(id: string): Promise<void> {
  return invoke("delete_page", { id });
}

/** @deprecated Use {@link deletePage} instead. */
export async function deleteConcept(id: string): Promise<void> {
  return deletePage(id);
}

export async function archivePage(id: string): Promise<void> {
  return invoke("archive_page", { id });
}

/** @deprecated Use {@link archivePage} instead. */
export async function archiveConcept(id: string): Promise<void> {
  return archivePage(id);
}

export async function searchPages(
  query: string,
  limit?: number,
): Promise<Page[]> {
  const pages = await invoke<Page[]>("search_pages", { query, limit: limit ?? 5 });
  return withDomainArray(pages);
}

/** @deprecated Use {@link searchPages} instead. */
export async function searchConcepts(
  query: string,
  limit?: number,
): Promise<Page[]> {
  return searchPages(query, limit);
}

export async function listPages(
  status?: string,
  domain?: string,
  limit?: number,
  offset?: number,
): Promise<Page[]> {
  const pages = await invoke<Page[]>("list_pages", { status, domain, limit, offset });
  return withDomainArray(pages);
}

/** @deprecated Use {@link listPages} instead. */
export async function listConcepts(
  status?: string,
  domain?: string,
  limit?: number,
  offset?: number,
): Promise<Page[]> {
  return listPages(status, domain, limit, offset);
}

/**
 * Explicit-browse sibling of {@link listPages}: same request, marked as a
 * human-initiated wiki browse so the daemon attaches `Page.truth` on each
 * result entry when its cutover is live.
 */
export async function listPagesExplicitBrowse(
  status?: string,
  domain?: string,
  limit?: number,
  offset?: number,
): Promise<Page[]> {
  const pages = await invoke<Page[]>("list_pages_explicit_browse", { status, domain, limit, offset });
  return withDomainArray(pages);
}

// ── Home delta feed ────────────────────────────────────────────────────

export interface RetrievalEvent {
  timestamp_ms: number;
  agent_name: string;
  query?: string | null;
  page_titles: string[];
  /** Stable page IDs corresponding 1:1 with page_titles. Empty on legacy events. */
  page_ids: string[];
  memory_snippets: string[];
}

export type PageChangeKind = "created" | "revised" | "merged";

/** @deprecated Use {@link PageChangeKind} instead. */
export type ConceptChangeKind = PageChangeKind;

export interface PageChange {
  page_id: string;
  title: string;
  change_kind: PageChangeKind;
  changed_at_ms: number;
}

/** @deprecated Use {@link PageChange} instead. */
export type ConceptChange = PageChange;

export async function listRecentRetrievals(limit?: number): Promise<RetrievalEvent[]> {
  return invoke<RetrievalEvent[]>("list_recent_retrievals", { limit: limit ?? 10 });
}

export async function listRecentChanges(limit?: number): Promise<PageChange[]> {
  return invoke<PageChange[]>("list_recent_changes", { limit: limit ?? 10 });
}

export type ActivityKind = "concept" | "memory";

// Discriminated-union — matches Rust's internally-tagged serde enum.
export type ActivityBadge =
  | { kind: "new" }
  | { kind: "revised" }
  | { kind: "refined" }
  | { kind: "growing"; added: number }
  | { kind: "needs_review" }
  | { kind: "none" };

export interface RecentActivityItem {
  kind: ActivityKind;
  id: string;
  title: string;
  snippet: string | null;
  timestamp_ms: number;
  badge: ActivityBadge;
}

export async function listRecentMemories(
  limit: number,
  sinceMs?: number,
): Promise<RecentActivityItem[]> {
  return invoke<RecentActivityItem[]>("list_recent_memories", {
    limit,
    sinceMs: sinceMs ?? null,
  });
}

export async function listUnconfirmedMemories(
  limit: number = 6,
): Promise<RecentActivityItem[]> {
  return invoke<RecentActivityItem[]>("list_unconfirmed_memories", { limit });
}

export async function listRecentPages(
  limit: number,
  sinceMs?: number,
): Promise<RecentActivityItem[]> {
  return invoke<RecentActivityItem[]>("list_recent_pages", {
    limit,
    sinceMs: sinceMs ?? null,
  });
}

/** @deprecated Use {@link listRecentPages} instead. */
export async function listRecentConcepts(
  limit: number,
  sinceMs?: number,
): Promise<RecentActivityItem[]> {
  return listRecentPages(limit, sinceMs);
}

export interface ExportStats {
  exported: number;
  skipped: number;
  failed: number;
}

export interface ExportPageResponse {
  path: string;
}

export async function exportPagesToObsidian(
  vaultPath: string,
): Promise<ExportStats> {
  return invoke("export_pages_to_obsidian", { vaultPath });
}

/** @deprecated Use {@link exportPagesToObsidian} instead. */
export async function exportConceptsToObsidian(
  vaultPath: string,
): Promise<ExportStats> {
  return exportPagesToObsidian(vaultPath);
}

export async function exportPageToObsidian(
  pageId: string,
  vaultPath: string,
): Promise<ExportPageResponse> {
  return invoke("export_page_to_obsidian", { pageId, vaultPath });
}

/** @deprecated Use {@link exportPageToObsidian} instead. */
export async function exportConceptToObsidian(
  conceptId: string,
  vaultPath: string,
): Promise<ExportPageResponse> {
  return exportPageToObsidian(conceptId, vaultPath);
}

/**
 * The typed rejection `export_pages_as_okf` returns, before sending anything,
 * when the background service predates OKF export (app/src/search.rs
 * `OKF_EXPORT_ERROR_DAEMON_TOO_OLD`). Such a service would write an Obsidian
 * export into the folder and still report success.
 */
export const OKF_EXPORT_ERROR_DAEMON_TOO_OLD = "okf-export:daemon-too-old";

export function isOkfExportDaemonTooOld(error: unknown): boolean {
  const text = error instanceof Error ? error.message : error;
  return text === OKF_EXPORT_ERROR_DAEMON_TOO_OLD;
}

/**
 * Write every page, across all Spaces, as an OKF v0.2 bundle into `targetDir`
 * (an absolute folder path). The daemon decides whether the folder is safe to
 * write: a non-empty folder that is not an earlier Wenlan OKF export, or a
 * truth cutover in progress, rejects with a 409 whose sentence
 * {@link daemonErrorMessage} extracts. A service too old to write OKF rejects
 * with {@link OKF_EXPORT_ERROR_DAEMON_TOO_OLD} instead.
 */
export async function exportPagesAsOkf(targetDir: string): Promise<ExportStats> {
  return invoke("export_pages_as_okf", { targetDir });
}

// ===== Knowledge Directory =====

export async function getKnowledgePath(): Promise<string> {
  return invoke("get_knowledge_path");
}

export async function countKnowledgeFiles(): Promise<number> {
  return invoke("count_knowledge_files");
}

// ===== Profile =====

export async function getProfile(): Promise<Profile | null> {
  return invoke<Profile | null>("get_profile");
}

export async function updateProfile(
  id: string,
  name?: string,
  displayName?: string,
  email?: string,
  bio?: string,
  avatarPath?: string,
): Promise<void> {
  return invoke("update_profile", {
    id,
    name: name ?? null,
    display_name: displayName ?? null,
    email: email ?? null,
    bio: bio ?? null,
    avatar_path: avatarPath ?? null,
  });
}

// ===== Agents =====

export async function listAgents(): Promise<AgentConnection[]> {
  return invoke<AgentConnection[]>("list_agents");
}

export async function getAgent(name: string): Promise<AgentConnection | null> {
  return invoke<AgentConnection | null>("get_agent", { name });
}

export async function updateAgent(
  name: string,
  updates: {
    agentType?: string;
    description?: string;
    enabled?: boolean;
    trustLevel?: string;
    /** Pass empty string to clear the display_name. */
    displayName?: string;
  },
): Promise<void> {
  return invoke("update_agent", {
    name,
    agent_type: updates.agentType ?? null,
    description: updates.description ?? null,
    enabled: updates.enabled ?? null,
    trust_level: updates.trustLevel ?? null,
    display_name: updates.displayName ?? null,
  });
}

export async function deleteAgent(name: string): Promise<void> {
  return invoke("delete_agent", { name });
}

// ===== Pin / Unpin =====

export async function pinMemory(sourceId: string): Promise<void> {
  return invoke("pin_memory", { sourceId });
}

export async function unpinMemory(sourceId: string): Promise<void> {
  return invoke("unpin_memory", { sourceId });
}

export async function listPinnedMemories(): Promise<MemoryItem[]> {
  const memories = await invoke<MemoryItem[]>("list_pinned_memories");
  return withDomainArray(memories);
}

// ===== Import =====

export interface ImportResult {
  imported: number;
  skipped: number;
  breakdown: Record<string, number>;
  entities_created: number;
  observations_added: number;
  relations_created: number;
  batch_id: string;
}

export async function importMemories(
  source: string,
  content: string,
  label?: string,
  chunk?: { batchId: string; chunkIndex: number; chunkTotal: number },
): Promise<ImportResult> {
  return invoke("import_memories_cmd", {
    source,
    content,
    label: label ?? null,
    batchId: chunk?.batchId ?? null,
    chunkIndex: chunk?.chunkIndex ?? null,
    chunkTotal: chunk?.chunkTotal ?? null,
  });
}

// ===== Import batches (honest phased progress) =====
//
// One import is a batch: the client generates a batch id up front, uploads the
// parsed memories in chunks, and then reads live row counts back through
// `import_batch_status_cmd`. `done`/`total` are rows, never elapsed time.
// `distill` never reports a usable `total` (always 0): render it as a live
// count, never as a bar.

export type ImportPhase = "ingest" | "store" | "detect" | "enrich" | "link" | "distill";

export type ImportPhaseState = "pending" | "running" | "complete" | "failed";

export interface ImportPhaseStatus {
  phase: ImportPhase;
  state: ImportPhaseState;
  /** Units finished — memories for every phase except `distill`, which counts pages. */
  done: number;
  /** Units expected. 0 means not yet known: render as waiting, never as 0%. */
  total: number;
  /** Units that failed and will not be retried. */
  failed: number;
}

export interface ImportBatchStatus {
  batch_id: string;
  /** The `import_source` the memories were written with (chatgpt, claude, other). */
  source: string;
  started_at: number;
  updated_at: number;
  /** Import requests received so far for this batch. */
  chunks_received: number;
  memories_imported: number;
  memories_skipped: number;
  /** Entities this batch's memories link to, by lifecycle state. */
  entities_detected: number;
  entities_established: number;
  pages_distilled: number;
  phases: ImportPhaseStatus[];
  /** Every background phase has settled (complete or failed). */
  complete: boolean;
  space?: string | null;
}

export interface ActiveImportBatchesResponse {
  batches: ImportBatchStatus[];
}

/** Memories per `import_memories_cmd` call. n memories produce ceil(n/500) calls. */
export const IMPORT_CHUNK_SIZE = 500;

export async function getImportBatchStatus(batchId: string): Promise<ImportBatchStatus> {
  return invoke("import_batch_status_cmd", { batchId });
}

export async function getActiveImportBatches(): Promise<ActiveImportBatchesResponse> {
  return invoke("active_import_batches_cmd");
}

// ===== Chat Export Import =====

export interface ImportChatExportResponse {
  import_id: string;
  vendor: string;
  conversations_total: number;
  conversations_new: number;
  conversations_skipped_existing: number;
  memories_stored: number;
}

export async function importChatExport(
  path: string,
): Promise<ImportChatExportResponse> {
  return invoke<ImportChatExportResponse>("import_chat_export", { path });
}

export async function saveTempFile(bytes: Uint8Array, filename: string): Promise<string> {
  return invoke<string>("save_temp_file", { bytes: Array.from(bytes), filename });
}

/** User-facing labels for import pipeline stages. Reusable across any UI
 *  that references import state (settings, home, notifications, etc.). */
export const IMPORT_STAGE_LABELS: Record<string, string> = {
  parsing: "Reading archive",
  stage_a: "Importing conversations",
  stage_b: "Classifying and extracting entities",
  done: "Complete",
  error: "Failed",
};

/** Get a user-facing label for an import stage. Falls back to the raw value. */
export function importStageLabel(stage: string): string {
  return IMPORT_STAGE_LABELS[stage] ?? stage;
}

export interface PendingImport {
  id: string;
  vendor: string;
  stage: string;
  source_path: string;
  processed_conversations: number;
  total_conversations: number | null;
}

export async function listPendingImports(): Promise<PendingImport[]> {
  return invoke<PendingImport[]>("list_pending_imports");
}

// ===== Avatar =====

export async function setAvatar(sourcePath: string): Promise<string> {
  return invoke("set_avatar", { sourcePath });
}

export async function getAvatarDataUrl(): Promise<string | null> {
  return invoke<string | null>("get_avatar_data_url");
}

export async function removeAvatar(): Promise<void> {
  return invoke("remove_avatar");
}

// ===== Setup Wizard =====

// `Reading` and its predicates live in ./reading, NOT here: component
// tests mock this whole module, and a rule about failed measurements must
// not vanish with the mock. The type is re-exported so wire consumers can
// keep importing it from one place (types are erased, so the re-export
// survives `vi.mock`).
export type { Reading } from "./reading";
import type { Reading } from "./reading";

/** How a client is present on this machine. `detected` is the superset
 *  ("installed" or "config_only"); this says which. `config_only` means no
 *  program was found at any known location, only the client's config or home
 *  folder: a leftover, or a program installed somewhere unusual. */
export type McpInstallState =
  | { kind: "installed" }
  | { kind: "config_only" }
  | { kind: "not_found" }
  | { kind: "unreadable"; error: string };

export type McpRepairReason =
  | "command_missing"
  | "command_not_found"
  | "command_not_runnable"
  | "args_invalid";

/** Whether the raw `wenlan` entry in the client's own config would launch.
 *  `needs_repair` is fixed by the existing `write_mcp_config` command. */
export type McpEntryHealth =
  | { kind: "no_entry" }
  | { kind: "healthy" }
  | { kind: "needs_repair"; reason: McpRepairReason; detail: string }
  | { kind: "unreadable"; error: string };

export interface McpClient {
  name: string;
  client_type: string;
  /** Null when the directory this client's config hangs off could not be
   *  determined — there is no path to show, and no honest placeholder. */
  config_path: string | null;
  detected: Reading;
  already_configured: Reading;
  /** The file half of `already_configured`: a raw `wenlan`/`origin` entry. */
  has_raw_entry: Reading;
  /** Both a `wenlan` AND a legacy `origin` raw entry. */
  has_raw_duplicate: Reading;
  /** The plugin half of `already_configured`. */
  has_plugin: Reading;
  /** Optional until every mock supplies it; the app always sends it. */
  install_state?: McpInstallState;
  /** Optional until every mock supplies it; the app always sends it. */
  entry_health?: McpEntryHealth;
}

export async function shouldShowWizard(): Promise<boolean> {
  return invoke("should_show_wizard");
}

export async function getSetupCompleted(): Promise<boolean> {
  return invoke("get_setup_completed");
}

export async function setSetupCompleted(completed: boolean): Promise<void> {
  return invoke("set_setup_completed", { completed });
}

export async function detectMcpClients(): Promise<McpClient[]> {
  return invoke("detect_mcp_clients_cmd");
}

/** Writes the raw `wenlan` MCP entry into `clientType`'s config file.
 *
 *  RESOLVES WITH the resolver inputs that could NOT be determined while the
 *  command it wrote was chosen. An empty array is itself a measurement — every
 *  input was read. A non-empty one means the write went through off a search
 *  that never built some of its candidates, and the caller must say so rather
 *  than render a plain success. This used to resolve with `void`, so the two
 *  were the same event to every caller (round 6, D3's boundary defect). */
export async function writeMcpConfig(clientType: string): Promise<UndeterminedInput[]> {
  return invoke("write_mcp_config", { clientType });
}

/** Removes the raw `wenlan`/legacy `origin` MCP entry from `clientType`'s
 *  config file — the Diagnostics fix for a double registration (a plugin plus
 *  a raw entry). Leaves sibling servers and unrelated config intact. Rejects
 *  with a plain error if the file is missing or has no such entry. */
export async function removeRawMcpEntry(clientType: string): Promise<void> {
  return invoke("remove_raw_mcp_entry", { clientType });
}

/** Removes ONLY the legacy `origin` MCP entry from `clientType`'s config,
 *  keeping the live `wenlan` entry — the Diagnostics fix for a raw+raw
 *  duplicate (both `wenlan` and `origin` present, on a no-plugin client).
 *  Unlike removeRawMcpEntry, it never drops `wenlan`. Rejects with a plain
 *  error if the file is missing or has no `origin` entry. */
export async function removeLegacyMcpEntry(clientType: string): Promise<void> {
  return invoke("remove_legacy_mcp_entry", { clientType });
}

export interface WenlanMcpEntry {
  command: string;
  args: string[];
}

/** The entry, plus the resolver inputs that could not be determined while
 *  choosing it. Kept as a wrapper rather than extra fields on
 *  `WenlanMcpEntry`, because that shape is written verbatim into the user's
 *  client config file and must stay exactly `{command, args}`. */
export interface WenlanMcpEntryReport {
  entry: WenlanMcpEntry;
  /** Empty means every input was read. Non-empty means this command was
   *  chosen by a search that did not cover the paths it looks like it
   *  covered — indistinguishable from a complete search before round 6. */
  undetermined: UndeterminedInput[];
}

/** Returns the `wenlan` MCP server entry (command + args) with real values —
 *  either a resolved local binary path (dev) or `npx -y wenlan-mcp` (prod) —
 *  together with anything the search could not determine. */
export async function getWenlanMcpEntry(): Promise<WenlanMcpEntryReport> {
  return invoke("get_wenlan_mcp_entry");
}

export type PluginInstallClientType = "claude_code" | "codex_cli";

/** Installs the Wenlan plugin for `clientType` (marketplace add, then
 *  plugin install/add) by shelling out to that client's CLI — never writes
 *  a raw MCP entry (`~/.claude.json` or `[mcp_servers.wenlan]`), since the
 *  plugin registers its own MCP server. Idempotent: resolves if the
 *  marketplace or plugin is already present. Rejects with a plain error
 *  message ("Claude Code CLI not found" / "Codex CLI not found", a step
 *  failure, or "unsupported client type") — callers must not block the
 *  wizard on that rejection. */
export async function installClientPlugin(clientType: PluginInstallClientType): Promise<void> {
  return invoke("install_client_plugin", { clientType });
}

/** Tell the daemon the user connected `clientType` when setup found it already
 *  configured (so neither a config write nor a plugin install ran). Resolves
 *  to whether the daemon accepted the row; false for an unknown client or a
 *  daemon that is down or slow. Best-effort: it never rejects on those. */
export async function registerConnectedAgent(clientType: string): Promise<boolean> {
  return invoke("register_connected_agent", { clientType });
}

// ===== Wire State (real, resolved wiring — wizard progress + Diagnostics) =====

/** Whether the daemon this app spawned sits in the app's kill-on-close job
 *  object — i.e. whether a hard kill of the app really ends it. `unbound`
 *  carries why the binding failed; `not_supported` is a platform without job
 *  objects. */
export type JobBinding =
  | { state: "bound" }
  | { state: "unbound"; reason: string }
  | { state: "not_supported" };

/** What `stop_sidecar` established about the daemon it was asked to stop.
 *  `ended` is the only outcome that means the guarantee held; an identity that
 *  could not be captured lands on `could_not_measure`, never on `ended`. */
export type SidecarStopOutcome =
  | { outcome: "no_sidecar" }
  | { outcome: "ended" }
  | { outcome: "still_running"; reason: string }
  | { outcome: "could_not_measure"; reason: string };

export interface DaemonWire {
  base_url: string;
  reachable: boolean;
  version: string | null;
  error: string | null;
  /** `null`/absent means this app owns no sidecar (a service holds the daemon,
   *  or it exited). It is never a stand-in for `{ state: "bound" }`. */
  sidecar_job_binding?: JobBinding | null;
  /** True when this app started a sidecar without being able to measure
   *  whether launchd already owned the daemon — two daemons are then possible
   *  for a reason nothing else on this surface would show. */
  sidecar_spawned_on_unknown_owner: boolean;
  /** Outcome of the most recent sidecar stop, or `null` if the app has not
   *  tried to stop one in this process. `null` is not `ended`. */
  last_sidecar_stop?: SidecarStopOutcome | null;
}

/** What a `wenlan-mcp` candidate path turned out to be. `absent` is the only
 *  measured absence; `unreadable` is a failed look and must never be read as
 *  one. `not_a_file` is a directory (or similar) squatting on the name, and
 *  `not_executable` is a real file that cannot be a program — empty, or (on
 *  Unix) with no execute bit. Both of those are MEASURED negatives, same
 *  standing as `absent`. */
export type CandidateProbe =
  | { kind: "file" }
  | { kind: "not_a_file" }
  | { kind: "not_executable"; reason: string }
  | { kind: "absent" }
  | { kind: "unreadable"; error: string };

export interface BinaryCandidate {
  path: string;
  /** The measurement. There is deliberately no `exists: boolean` beside it:
   *  that boolean answered `false` for "absent", for "a directory squats on
   *  the name" and for "the OS refused to look", and every reader that took it
   *  inherited the conflation this trail exists to remove. Match on `kind`. */
  state: CandidateProbe;
  source: "WENLAN_MCP_DEV_BIN" | "installed" | "bundled" | "cargo";
}

/** A candidate the OS would not answer about, with its error. */
export interface UnreadableCandidate {
  path: string;
  error: string;
}

export interface BinaryWire {
  /** The command setup would write — or `null` when the binary search could
   *  not be completed, in which case setup writes NOTHING and leaves the
   *  user's existing config alone. Render the null; there is no honest
   *  placeholder, because no command was chosen. */
  command: string | null;
  args: string[];
  /** Set exactly when `command` is null: why nothing could be resolved.
   *  `unreadable` is candidate PATHS the OS refused to answer about. */
  unresolved?: {
    message: string;
    unreadable: UnreadableCandidate[];
  };
  /** Inputs the candidate paths hang off that could not be determined at all,
   *  so those candidates were never built and have NO entry in `candidates`.
   *  A short `candidates` list is therefore not evidence the search was short —
   *  read this for that.
   *
   *  Sits beside `command`, NOT inside `unresolved`, and that placement is the
   *  point: it used to be reachable only when `command` was null, which made an
   *  undetermined input impossible to report on a search that FOUND something —
   *  exactly when it is invisible, because the command reads as a complete
   *  answer. Non-empty beside a command means "this command was chosen, and
   *  these inputs were never read". */
  undetermined: UndeterminedInput[];
  candidates: BinaryCandidate[];
}

/** An input the candidate paths hang off that could not be determined — the
 *  home directory, the app's own directory, `WENLAN_MCP_DEV_BIN`. `blocked`
 *  names the candidate sources that were never probed because of it. */
export interface UndeterminedInput {
  input: string;
  blocked: string;
  error: string;
}

export interface ClientWire {
  client_type: string;
  name: string;
  /** Three-valued — see [`Reading`]. A client the app could not look at is
   *  NOT a client that is absent, and this card is the one surface whose
   *  whole job is telling a user which of the two they are looking at. */
  detected: Reading;
  /** Null when the config directory could not be determined. */
  config_path: string | null;
  has_raw_entry: Reading;
  /** Config holds BOTH a `wenlan` and a legacy `origin` raw entry — the
   *  raw+raw duplicate. For a no-plugin client (Cursor, Gemini CLI) this is
   *  the only signal of a duplicate, and it routes to `removeLegacyMcpEntry`,
   *  which removes only `origin`. */
  has_raw_duplicate: Reading;
  has_plugin: Reading;
  /** `unknown` is the route for a client whose detection or plugin state could
   *  not be read: there is no instruction to give, and naming one would be an
   *  instruction derived from a failed measurement. */
  route: "plugin" | "config" | "skip" | "unknown";
}

export interface WireState {
  daemon: DaemonWire;
  mcp_binary: BinaryWire;
  clients: ClientWire[];
}

/** The real, resolved wiring of Wenlan on this machine: daemon
 *  reachability, the `wenlan-mcp` binary that would actually be written into
 *  a client config (with the full candidate trail — missing paths included,
 *  never silently dropped), and per-client MCP routing. Never rejects on a
 *  down daemon (`daemon.reachable: false` instead). */
export async function getWireState(): Promise<WireState> {
  return invoke("wire_state");
}

/** Outcome of an on-demand daemon start. Discriminated by `status`: the daemon
 *  was already up (`already_running`), launchd owns it (`launchd_managed`), we
 *  spawned it (`started`), or we could not (`failed`, with a reason to show). */
export type DaemonStartResult =
  | { status: "started" }
  | { status: "already_running" }
  | { status: "launchd_managed" }
  | { status: "failed"; message: string };

/** Start the wenlan-server sidecar if nothing already serves it — the healing
 *  action behind Diagnostics' daemon-down state. Guarded app-side: a live port
 *  or a launchd-managed daemon is never double-spawned. */
export async function startDaemonSidecar(): Promise<DaemonStartResult> {
  return invoke("start_daemon_sidecar");
}

// ===== Onboarding Journey Milestones =====

export type MilestoneId =
  | "intelligence-ready"
  | "first-memory"
  | "first-recall"
  | "first-concept"
  | "graph-alive"
  | "second-agent";

export interface MilestoneRecord {
  id: MilestoneId;
  first_triggered_at: number;
  acknowledged_at: number | null;
  payload: Record<string, unknown> | null;
}

export async function listOnboardingMilestones(): Promise<MilestoneRecord[]> {
  return invoke("list_onboarding_milestones");
}

export async function acknowledgeOnboardingMilestone(id: MilestoneId): Promise<void> {
  return invoke("acknowledge_onboarding_milestone", { id });
}

export async function resetOnboardingMilestones(): Promise<void> {
  return invoke("reset_onboarding_milestones");
}

// ===== Briefing & Contradictions =====

export interface BriefingResponse {
  content: string;
  new_today: number;
  primary_agent: string | null;
  generated_at: number;
  is_stale: boolean;
}

export interface ContradictionItem {
  id: string;
  new_content: string;
  existing_content: string;
  new_source_id: string;
  existing_source_id: string;
}

export async function getBriefing(): Promise<BriefingResponse> {
  return invoke("get_briefing");
}

export async function getPendingContradictions(): Promise<ContradictionItem[]> {
  return invoke("get_pending_contradictions");
}

// ===== Profile Narrative =====

export interface NarrativeResponse {
  content: string;
  generated_at: number;
  is_stale: boolean;
  memory_count: number;
}

export async function getProfileNarrative(): Promise<NarrativeResponse> {
  return invoke("get_profile_narrative");
}

export async function regenerateNarrative(): Promise<NarrativeResponse> {
  return invoke("regenerate_narrative");
}

// ===== Agent Activity =====

export interface AgentActivityItem {
  id: number;
  timestamp: number;
  agent_name: string;
  action: string;
  memory_ids: string | null;
  query: string | null;
  detail: string | null;
  memory_titles: string[];
}

export async function listAgentActivity(
  limit?: number,
  agentName?: string,
  since?: number,
): Promise<AgentActivityItem[]> {
  return invoke("list_agent_activity", {
    limit: limit ?? 50,
    agentName: agentName ?? null,
    since: since ?? null,
  });
}

// ── Remote Access ──────────────────────────────────────────────────

export type RemoteAccessStatus =
  | { status: "off" }
  | { status: "starting" }
  | { status: "connected"; tunnel_url: string | null; relay_url: string | null }
  | { status: "error"; error: string };

export async function toggleRemoteAccess(
  enabled: boolean,
  expectedRevision?: string,
): Promise<RemoteAccessStatus> {
  return invoke<RemoteAccessStatus>("toggle_remote_access", { enabled, expectedRevision: expectedRevision ?? null });
}

export async function reconnectRemoteAccess(expectedRevision: string): Promise<RemoteAccessStatus> {
  return invoke<RemoteAccessStatus>("reconnect_remote_access", { expectedRevision });
}

export interface RemoteAccessProfile {
  revision: string;
  space: string;
  enabled: boolean;
  disconnect_pending: boolean;
  credential_expires_at: number | null;
}

export async function getRemoteAccessProfile(): Promise<RemoteAccessProfile | null> {
  return invoke<RemoteAccessProfile | null>("get_remote_access_profile");
}

/** What Web access shares: every Space plus everything not in one, or one Space. */
export type RemoteScope = { kind: "wholeLibrary" } | { kind: "space"; name: string };

export async function configureRemoteAccess(scope: RemoteScope, expectedRevision?: string): Promise<RemoteAccessProfile> {
  return invoke<RemoteAccessProfile>("configure_remote_access", { scope, expectedRevision: expectedRevision ?? null });
}

export async function getRemoteAccessStatus(): Promise<RemoteAccessStatus> {
  return invoke<RemoteAccessStatus>("get_remote_access_status");
}

export interface RemotePairing {
  pairingId: string;
  clientId: string;
  resource: string;
  scopes: string[];
  expiresAt: number;
  /** Fields the relay added later. Absent from an older relay. */
  clientName?: string | null;
  redirectHost?: string | null;
  knownClient?: boolean;
}

/** What an allowed app may do: search and read, or also add, edit and delete. */
export type RemoteAccess = "query" | "write";

export type RemoteGrantEndReason = 'revoked' | 'reset' | 'expired' | 'replaced';

export interface RemoteGrant {
  id: string;
  clientId: string;
  space: string;
  createdAt: number;
  expiresAt: number;
  status: 'active' | 'inactive';
  cleanupPending: boolean;
  /** Fields the relay added later. Absent from an older relay. */
  clientName?: string | null;
  redirectHost?: string | null;
  knownClient?: boolean;
  lastUsedAt?: number | null;
  /** Only on an inactive grant. An unknown reason arrives as "other". */
  endReason?: RemoteGrantEndReason | 'other' | null;
  /** A relay that predates the choice sends nothing, which means "query". */
  access?: RemoteAccess;
}

export interface RemoteGrantPage { items: RemoteGrant[]; cursor: string | null }
export interface RemoteGrantRevocation { revoked: boolean; cleanupPending: boolean }

export async function inspectRemotePairing(expectedRevision: string, pairingId: string): Promise<RemotePairing> {
  return invoke("inspect_remote_pairing", { expectedRevision, pairingId });
}
/** Finds a pending request from the short code the browser page shows. */
export async function lookupRemotePairing(expectedRevision: string, code: string): Promise<RemotePairing> {
  return invoke("lookup_remote_pairing", { expectedRevision, code });
}
/** Tells the browser the person declined, so it stops waiting. */
export async function denyRemotePairing(expectedRevision: string, pairingId: string): Promise<void> {
  return invoke("deny_remote_pairing", { expectedRevision, pairingId });
}
/** `access` is the person's choice in the dialog; search only unless they chose write. */
export async function approveRemotePairing(expectedRevision: string, inspected: RemotePairing, access: RemoteAccess = "query"): Promise<void> {
  return invoke("approve_remote_pairing", { expectedRevision, inspected, access });
}
export async function listRemoteGrants(expectedRevision: string, cursor: string | null = null): Promise<RemoteGrantPage> {
  return invoke("list_remote_grants", { expectedRevision, cursor });
}
export async function revokeRemoteGrant(expectedRevision: string, grantId: string): Promise<RemoteGrantRevocation> {
  return invoke("revoke_remote_grant", { expectedRevision, grantId });
}
/** Takes the pairing code from the last `wenlan://pair` link, if one is waiting. */
export async function takeRemotePairingLink(): Promise<string | null> {
  return invoke("take_remote_pairing_link");
}

/**
 * Native check that the protected local backend answers and that the relay
 * reports this app's own connection as live.
 */
export interface RemoteConnectionTest {
  ok: boolean;
  latency_ms: number | null;
  error: string | null;
}

/** Does not prove a ChatGPT/Claude OAuth conversation or a tool call. */
export async function testRemoteMcpConnection(): Promise<RemoteConnectionTest> {
  return invoke<RemoteConnectionTest>("test_remote_mcp_connection");
}

/** Gives this computer a fresh 90-day key. Apps connected before must connect again. */
export async function renewRemoteAccess(expectedRevision: string): Promise<RemoteAccessStatus> {
  return invoke<RemoteAccessStatus>("renew_remote_access", { expectedRevision });
}

/** Why Web access stopped on its own. The wire carries a kind, never copy. */
export interface RemoteAccessNotice {
  kind: "stopped" | "expired";
}

/** Takes the pending notice, if any. A notice is returned once. */
export async function takeRemoteAccessNotice(): Promise<RemoteAccessNotice | null> {
  return invoke<RemoteAccessNotice | null>("take_remote_access_notice");
}

// ── Nurturing Garden ────────────────────────────────────────────────

export async function getNurtureCards(limit?: number, domain?: string): Promise<MemoryItem[]> {
  const memories = await invoke<MemoryItem[]>("get_nurture_cards_cmd", {
    limit: limit ?? 3,
    domain: domain ?? null,
  });
  return withDomainArray(memories);
}

export async function setStability(sourceId: string, stability: "new" | "learned" | "confirmed"): Promise<void> {
  return invoke("set_stability_cmd", { sourceId, stability });
}

export async function correctMemory(sourceId: string, correctionPrompt: string): Promise<string> {
  return invoke("correct_memory_cmd", { sourceId, correctionPrompt });
}

// ── Decision Log ───────────────────────────────────────────────────

export async function listDecisions(
  domain?: string,
  limit?: number,
): Promise<MemoryItem[]> {
  const memories = await invoke<MemoryItem[]>("list_decisions_cmd", {
    domain: domain ?? null,
    limit: limit ?? null,
  });
  return withDomainArray(memories);
}

export async function listDecisionDomains(): Promise<string[]> {
  return invoke("list_decision_domains_cmd", {});
}

// ── Lifecycle / Run-at-login ───────────────────────────────────────

export async function isRunAtLoginEnabled(): Promise<boolean> {
  return invoke("is_run_at_login_enabled");
}

export async function setRunAtLogin(enabled: boolean): Promise<void> {
  return invoke("set_run_at_login", { enabled });
}

// ── Product telemetry consent ─────────────────────────────────────────

/** Daemon-owned, opt-in product telemetry status. */
export interface TelemetryStatus {
  enabled: boolean;
  available: boolean;
  pending_operations: number;
}

export async function getTelemetryStatus(): Promise<TelemetryStatus> {
  return invoke<TelemetryStatus>("get_telemetry_status");
}

export async function setTelemetryEnabled(enabled: boolean): Promise<TelemetryStatus> {
  return invoke<TelemetryStatus>("set_telemetry_enabled", { enabled });
}

export async function quitWenlanFull(): Promise<void> {
  return invoke("quit_wenlan_full");
}

export async function acknowledgeGuardedQuitRequest(
  requestId: number,
  deliveryId: number,
): Promise<boolean> {
  return invoke("acknowledge_guarded_quit_request", { requestId, deliveryId });
}

export async function cancelGuardedQuitRequest(
  requestId: number,
  deliveryId: number,
): Promise<boolean> {
  return invoke("cancel_guarded_quit_request", { requestId, deliveryId });
}

// ── Page map ─────────────────────────────────────────────────────────
// Wire shapes hand-mirrored from `crates/wenlan-types/src/page_map.rs`, which
// is the daemon's own contract and now lives in this workspace. The Rust bridge
// speaks `serde_json::Value` so it can hand the frontend whatever the daemon
// sent (see the note in app/src/api.rs), which makes these interfaces the only
// typing the UI gets — keep them in step with that file field for field.

export type PageMapRefKind = "memory" | "entity" | "page" | "section" | "idea";
export type PageMapStatus = "suggested" | "active" | "dismissed";

export interface PageMapNode {
  id: string;
  parent_id: string | null; // null only for the root
  rank: number;
  ref_kind: PageMapRefKind;
  ref_id: string;
  label: string | null; // null = render from the backing object
  status: PageMapStatus;
  pinned: boolean;
  placed: boolean;
  collapsed: boolean;
  x: number | null;
  y: number | null;
  width: number | null;
  height: number | null;
  ref_state: "live" | "dangling";
}

export interface PageMapEdge {
  id: string;
  from_node: string;
  to_node: string;
  kind: "link" | "suggested";
  label: string | null;
  status: PageMapStatus;
}

export interface PageMapViewport {
  x: number;
  y: number;
  zoom: number;
}

export interface PageMap {
  page_id: string;
  revision: number;
  map_schema: number;
  /** Missing on runtimes before independent idea nodes were introduced. */
  independent_ideas?: boolean;
  viewport?: PageMapViewport | null; // omitted by the daemon when unset
  nodes: PageMapNode[];
  edges: PageMapEdge[];
}

export interface PageMapNodeMutation {
  revision: number;
  node: PageMapNode;
}

export interface PageMapLayoutPosition {
  node_id: string;
  x: number;
  y: number;
  width: number;
  height: number;
  collapsed: boolean;
}

export interface PageMapApiError {
  status: number;
  message: string;
}

/** Decodes the `{"status":N,"error":"..."}` payload page-map commands reject
 *  with. Returns null for transport/parse failures, which callers treat as a
 *  generic error. */
export function asPageMapApiError(e: unknown): PageMapApiError | null {
  if (typeof e !== "string") return null;
  try {
    const parsed = JSON.parse(e);
    if (parsed && typeof parsed.status === "number") {
      return { status: parsed.status, message: String(parsed.error ?? "") };
    }
  } catch {
    // not JSON — transport/parse failure, treat as a generic error
  }
  return null;
}

export async function getPageMap(pageId: string): Promise<PageMap> {
  return invoke("get_page_map", { pageId });
}

export async function improvePageMap(pageId: string): Promise<PageMap> {
  return invoke("improve_page_map", { pageId });
}

export async function createPageMapNode(
  pageId: string,
  body: {
    base_revision: number;
    parent_id?: string | null;
    ref_kind: PageMapRefKind;
    ref_id: string;
    label?: string | null;
  },
): Promise<PageMapNodeMutation> {
  return invoke("create_page_map_node", { pageId, body });
}

export async function patchPageMapNode(
  pageId: string,
  nodeId: string,
  body: {
    base_revision: number;
    label?: string | null;
    pinned?: boolean;
    status?: PageMapStatus;
    rank?: number;
    parent_id?: string;
  },
): Promise<PageMapNodeMutation> {
  return invoke("patch_page_map_node", { pageId, nodeId, body });
}

export async function deletePageMapNode(
  pageId: string,
  nodeId: string,
  body: { base_revision: number },
): Promise<PageMapNodeMutation> {
  return invoke("delete_page_map_node", { pageId, nodeId, body });
}

export async function putPageMapLayout(
  pageId: string,
  body: {
    base_revision: number;
    viewport?: PageMapViewport | null;
    positions: PageMapLayoutPosition[];
  },
): Promise<PageMap> {
  return invoke("put_page_map_layout", { pageId, body });
}

// ── Communities (M6 cartography) ────────────────────────────────────────
// Wire shapes hand-mirrored from `crates/wenlan-types/src/communities.rs`
// (`community-read-v1`): the community read routes are merged to the daemon
// but the pinned wenlan-types release predates the crate module — same
// situation as the page-map types above.

export const COMMUNITY_READ_SCHEMA_VERSION = "community-read-v1";

export interface CommunitySummary {
  community_id: string;
  space: string;
  display_name: string | null;
  member_count: number;
  published_generation: number;
  algo_version: string;
  projection_version: string;
}

/** The daemon's resolved read scope, mirrored from app/src/api.rs
 *  `CommunityScope`: `{kind:"space", name}` for a registered Space, or
 *  `{kind:"global"|"uncategorized"}` with no name at all. */
export interface CommunityScope {
  kind: string;
  name?: string | null;
}

export interface CommunityListResponse {
  schema_version: string;
  scope?: CommunityScope | null;
  communities: CommunitySummary[];
  next_cursor: string | null;
}

export interface CommunityMember {
  space: string;
  node_id: string;
  node_kind: string;
  community_id: string;
  published_generation: number;
  attachment: string;
}

export interface CommunityMemberCursor {
  space: string;
  node_id: string;
}

export interface CommunityMembersResponse {
  schema_version: string;
  scope?: CommunityScope | null;
  members: CommunityMember[];
  next_cursor: CommunityMemberCursor | null;
}

export interface CommunityApiError {
  status: number;
  message: string;
}

/** Decodes the `{"status":N,"error":"..."}` payload community commands
 *  reject with (same envelope as `asPageMapApiError`). Returns null for
 *  transport/parse failures, which callers treat as a generic error. */
export function asCommunityApiError(e: unknown): CommunityApiError | null {
  if (typeof e !== "string") return null;
  try {
    const parsed = JSON.parse(e);
    if (parsed && typeof parsed.status === "number") {
      return { status: parsed.status, message: String(parsed.error ?? "") };
    }
  } catch {
    // not JSON — transport/parse failure, treat as a generic error
  }
  return null;
}

export async function listCommunities(
  space: string,
  cursor?: string | null,
  limit?: number,
): Promise<CommunityListResponse> {
  return invoke("list_communities", { space, cursor: cursor ?? null, limit: limit ?? null });
}

export async function listCommunityMembers(
  space: string,
  cursor?: CommunityMemberCursor | null,
  limit?: number,
): Promise<CommunityMembersResponse> {
  return invoke("list_community_members", { space, cursor: cursor ?? null, limit: limit ?? null });
}
