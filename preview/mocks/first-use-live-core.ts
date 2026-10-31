// SPDX-License-Identifier: AGPL-3.0-only
// Isolated-live first-use backend: the actual Main/Home/first-use/ImportView/
// PageDetail components against a scratch daemon, never the shared prod one.
//
// Scope contract (fail closed):
// - Reads: narrow allowlist of the commands the first-use surfaces mount.
//   Live reads reuse preview/mocks/live-invoke.ts HANDLERS; a few browser-
//   honest values are local (get_api_key has no keychain here; the browser
//   cannot mint review capabilities, so page_review_supported is honestly
//   "platform_unsupported", never a ready claim).
// - Mutations: import_memories_cmd and dismissing the first-page milestone, the point of the flow — and
//   only after proving the proxied daemon is the scratch one (GET
//   /api/knowledge/path equals the expected scratch path, /api/health ok).
//   Anything else (setup probes, model download, client wiring, authored page
//   writes, source registration, other milestone/review writes, unknown commands)
//   throws. There is no fixture fallback: a failed gate is an error, not
//   fixture data.
// - The daemon's HTTP surface exposes NO data-dir signal (verified against
//   crates/wenlan-server/src/routes.rs HealthResponse {status,
//   db_initialized, version}, StatusResponse, ConfigResponse, and
//   KnowledgePathResponse {path}): WENLAN_PREVIEW_DATA_DIR can only be
//   enforced at server startup (vite.first-use-live.config.ts refuses to
//   serve without it) plus whoever launches the daemon pointing it there.
//   The knowledge path is re-verified per mutation twice: in the vite
//   middleware before the import is proxied, and in the browser before it
//   is sent.
//
// Data flow: browser fetch("/daemon/...") → vite proxy → WENLAN_PREVIEW_DAEMON.
import { DEFAULTS, HANDLERS } from "./live-invoke";

export const FIRST_USE_LIVE_DAEMON = "http://127.0.0.1:17878";
export const FIRST_USE_LIVE_PORT = 1433;

declare const __WENLAN_PREVIEW_KNOWLEDGE_PATH__: string | undefined;

// --- startup gate (also imported by vite.first-use-live.config.ts) ---

export function isLoopbackHostname(hostname: string): boolean {
  const bare = hostname.trim().toLowerCase().replace(/^\[|\]$/g, "");
  return bare === "127.0.0.1" || bare === "::1" || bare === "localhost";
}

function fail(reason: string): never {
  throw new Error(`[first-use-live] ${reason}`);
}

/** Parse + normalize a daemon URL, or throw. No defaults, no fallback. */
export function normalizeDaemonUrl(value: unknown): string {
  if (typeof value !== "string" || !value.trim()) {
    fail(`WENLAN_PREVIEW_DAEMON is required (expected ${FIRST_USE_LIVE_DAEMON}).`);
  }
  let url: URL;
  try {
    url = new URL(value.trim().replace(/\/+$/, "") || "/");
  } catch {
    fail(`WENLAN_PREVIEW_DAEMON ${JSON.stringify(value)} is not a valid URL.`);
  }
  if (url.protocol !== "http:") {
    fail(`WENLAN_PREVIEW_DAEMON must be http (got ${url.protocol}).`);
  }
  if (!isLoopbackHostname(url.hostname)) {
    fail(`WENLAN_PREVIEW_DAEMON must be loopback (got ${url.hostname}).`);
  }
  if (!url.port) fail("WENLAN_PREVIEW_DAEMON must carry an explicit port.");
  if (Number(url.port) === 7878) {
    fail("WENLAN_PREVIEW_DAEMON must not be the shared prod daemon on :7878.");
  }
  if (url.pathname !== "/" && url.pathname !== "") {
    fail("WENLAN_PREVIEW_DAEMON must have no path component.");
  }
  return `http://${url.hostname.toLowerCase()}:${url.port}`;
}

/**
 * The live first-use server talks to exactly one scratch daemon. A different
 * loopback port is still a refusal: relax by changing FIRST_USE_LIVE_DAEMON,
 * not by widening this check at runtime.
 */
export function assertFirstUseLiveDaemon(value: unknown): string {
  const normalized = normalizeDaemonUrl(value);
  if (normalized !== FIRST_USE_LIVE_DAEMON) {
    fail(`WENLAN_PREVIEW_DAEMON must be exactly ${FIRST_USE_LIVE_DAEMON} (got ${normalized}).`);
  }
  return normalized;
}

export interface FirstUseLiveScratch {
  readonly dataDir: string;
  readonly knowledgePath: string;
}

export function normalizeScratchPath(value: unknown, name: string): string {
  if (typeof value !== "string" || !value.trim()) {
    fail(`${name} is required (absolute scratch path, no production library).`);
  }
  const trimmed = value.trim().replace(/\/+$/, "");
  if (!trimmed.startsWith("/")) fail(`${name} must be absolute (got ${JSON.stringify(value)}).`);
  return trimmed;
}

/** Both scratch locations must be explicit; the server refuses to start without them. */
export function assertFirstUseLiveScratchEnv(env: Record<string, string | undefined>): FirstUseLiveScratch {
  return {
    dataDir: normalizeScratchPath(env.WENLAN_PREVIEW_DATA_DIR, "WENLAN_PREVIEW_DATA_DIR"),
    knowledgePath: normalizeScratchPath(env.WENLAN_PREVIEW_KNOWLEDGE_PATH, "WENLAN_PREVIEW_KNOWLEDGE_PATH"),
  };
}

// --- HTTP-layer policy (mirrored by the vite guard middleware) ---

/** Read POSTs the first-use surfaces need (mirrors preview/devLivePolicy.ts shape). */
export const FIRST_USE_LIVE_READ_POSTS: ReadonlySet<string> = new Set([
  "/api/search",
  "/api/pages/search",
  "/api/memory/list",
  "/api/memory/entities/list",
  "/api/memory/entities/search",
]);

/** Permitted mutations: import and dismissing the generated first-page announcement. */
export const FIRST_USE_LIVE_MUTATION_POSTS: ReadonlySet<string> = new Set([
  "/api/import/memories",
  "/api/onboarding/milestones/first-concept/acknowledge",
]);

/** pathname (no query) + method gate for /daemon traffic. */
export function allowsFirstUseLiveRequest(method: string, path: string): boolean {
  if (!path.startsWith("/api/")) return false;
  if (method === "GET" || method === "HEAD") return true;
  if (method !== "POST") return false;
  return FIRST_USE_LIVE_READ_POSTS.has(path) || FIRST_USE_LIVE_MUTATION_POSTS.has(path);
}

// --- browser invoke boundary ---

/**
 * Read commands mounted by Main (search, memories, spaces), Home (stats,
 * pages, imports, milestones, provider, pending-review queue reads),
 * FirstUseGuide (pages, batches), ImportView polling, and PageDetail (page,
 * sources, links, revisions, entity). Every entry must exist in live-invoke
 * HANDLERS; the dispatch below throws if one goes missing instead of
 * stubbing it.
 */
export const FIRST_USE_LIVE_READS: ReadonlySet<string> = new Set([
  "list_pages",
  "search_pages",
  "get_page",
  "get_page_explicit_browse",
  "get_page_sources",
  "get_page_links",
  "get_page_revisions",
  "get_entity_detail_cmd",
  "get_memory_detail",
  "list_memories_by_ids",
  "list_memories_cmd",
  "get_memory_stats_cmd",
  "search",
  "search_entities_cmd",
  "list_entities_cmd",
  "list_spaces",
  "active_import_batches_cmd",
  "import_batch_status_cmd",
  "list_onboarding_milestones",
  "list_pending_revisions",
  "list_refinements",
  "list_unconfirmed_memories",
  "get_resolved_routing",
  "daemon_version",
  "get_truth_status",
]);

async function upstreamGet<T>(base: string, path: string): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`${base}${path}`, { method: "GET" });
  } catch (err) {
    throw new Error(
      `[first-use-live] scratch daemon unreachable at ${base}${path}: ` +
        (err instanceof Error ? err.message : String(err)),
    );
  }
  const text = await res.text();
  if (!res.ok) {
    throw new Error(`[first-use-live] GET ${base}${path} → ${res.status}${text ? `: ${text}` : ""}`);
  }
  return (text ? JSON.parse(text) : null) as T;
}

async function daemonGet<T>(path: string): Promise<T> {
  return upstreamGet<T>("/daemon", path);
}

export interface ScratchBinding {
  readonly health: unknown;
  readonly knowledgePath: unknown;
  readonly expectedKnowledgePath: string;
}

/**
 * Pure pre-mutation check. Health proves a live daemon answered; the
 * knowledge path proves it is the scratch one. Throws on any mismatch —
 * callers must not catch-and-continue into fixtures.
 */
export function verifyScratchBinding({ health, knowledgePath, expectedKnowledgePath }: ScratchBinding): void {
  const status = (health as { status?: unknown } | null)?.status;
  if (status !== "ok") {
    throw new Error("[first-use-live] scratch daemon is not healthy; refusing the import mutation.");
  }
  if (typeof knowledgePath !== "string" || !knowledgePath) {
    throw new Error("[first-use-live] daemon returned no knowledge path; refusing the import mutation.");
  }
  const actual = knowledgePath.replace(/\/+$/, "");
  const expected = expectedKnowledgePath.replace(/\/+$/, "");
  if (actual !== expected) {
    throw new Error(
      `[first-use-live] daemon knowledge path ${JSON.stringify(knowledgePath)} ` +
        `does not match scratch ${JSON.stringify(expectedKnowledgePath)}; refusing the import mutation.`,
    );
  }
}

export interface FirstUseLiveInvokeOpts {
  /** Test seam. Production reads the vite-injected define instead. */
  readonly expectedKnowledgePath?: string;
}

function readExpectedKnowledgePath(opts?: FirstUseLiveInvokeOpts): string {
  if (opts?.expectedKnowledgePath) return opts.expectedKnowledgePath;
  const injected = typeof __WENLAN_PREVIEW_KNOWLEDGE_PATH__ !== "undefined"
    ? __WENLAN_PREVIEW_KNOWLEDGE_PATH__
    : undefined;
  if (typeof injected === "string" && injected) return injected;
  fail("scratch knowledge path is not configured; serve through vite.first-use-live.config.ts.");
}

/**
 * Prove the daemon behind `base` is the scratch one: reachable, healthy,
 * and reporting the expected knowledge path. Shared by the browser gate
 * (base "/daemon", via the vite proxy) and the vite middleware gate (base
 * the absolute daemon target, checked before the import is proxied).
 */
export async function verifyUpstreamScratchBinding(
  base: string,
  expectedKnowledgePath: string,
): Promise<void> {
  const [health, knowledge] = await Promise.all([
    upstreamGet<{ status?: unknown }>(base, "/api/health"),
    upstreamGet<{ path?: unknown }>(base, "/api/knowledge/path"),
  ]);
  verifyScratchBinding({ health, knowledgePath: knowledge?.path, expectedKnowledgePath });
}

/** Re-prove scratch binding, then run the one permitted mutation. */
async function gatedImportMemories(args: unknown, opts?: FirstUseLiveInvokeOpts): Promise<unknown> {
  const expectedKnowledgePath = readExpectedKnowledgePath(opts);
  await verifyUpstreamScratchBinding("/daemon", expectedKnowledgePath);
  return HANDLERS.import_memories_cmd(args);
}

export async function invoke<T>(
  command: string,
  args?: Record<string, unknown>,
  opts?: FirstUseLiveInvokeOpts,
): Promise<T> {
  // Passive window-chrome stub: presentation only, claims nothing.
  if (command === "set_traffic_lights_visible") return null as T;
  // Passive native-window state only; grants no live-daemon write capability.
  if (command === "set_editor_composing") return null as T;
  // No wenlan://pair link can reach a browser preview.
  if (command === "take_remote_pairing_link") return null as T;
  if (command === "take_open_link") return null as T;
  // Nor can Web access stop on its own here, so no notice is ever waiting.
  if (command === "take_remote_access_notice") return null as T;
  // No keychain in the browser; the Anthropic-key flow is out of scope here.
  // External + on-device provider state below stays live.
  if (command === "get_api_key") return DEFAULTS.get_api_key as T;
  // Scratch daemons start with no registered sources, and this preview never
  // registers one (add_source throws); the merge with local config dirs the
  // native command performs has no browser equivalent.
  if (command === "list_registered_sources") return DEFAULTS.list_registered_sources as T;
  // Honest static: a browser harness cannot mint presence capabilities, so
  // review can never succeed here regardless of daemon version.
  if (command === "page_review_supported") return "platform_unsupported" as T;
  // Live provider reads (NOT live-invoke's synthesized setup rows): the real
  // daemon answers, so "no model configured" and a later real model both show.
  if (command === "get_external_llm") {
    const cfg = await daemonGet<{ external_llm_endpoint?: unknown; external_llm_model?: unknown }>("/api/config");
    return [cfg?.external_llm_endpoint ?? null, cfg?.external_llm_model ?? null] as T;
  }
  if (command === "get_on_device_model") {
    return (await daemonGet("/api/on-device-model")) as T;
  }
  if (command === "acknowledge_onboarding_milestone" && args?.id === "first-concept") {
    await verifyUpstreamScratchBinding("/daemon", readExpectedKnowledgePath(opts));
    const response = await fetch("/daemon/api/onboarding/milestones/first-concept/acknowledge", { method: "POST" });
    if (!response.ok) throw new Error(`[first-use-live] milestone acknowledgement failed: ${response.status}`);
    return undefined as T;
  }
  if (command === "import_memories_cmd") {
    return (await gatedImportMemories(args, opts)) as T;
  }
  if (FIRST_USE_LIVE_READS.has(command)) {
    const handler = HANDLERS[command];
    if (!handler) {
      fail(`read "${command}" lost its live handler; refusing to stub it.`);
    }
    return (await handler(args)) as T;
  }
  throw new Error(
    `[first-use-live] unsupported command "${command}": this preview allows ` +
      "first-use reads plus import and first-page acknowledgement only.",
  );
}

export { convertFileSrc, isTauri, Resource, Channel, PluginListener, addPluginListener, transformCallback } from "./core";
