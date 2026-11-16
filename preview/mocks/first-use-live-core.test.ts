// SPDX-License-Identifier: AGPL-3.0-only
// Focused checks for the isolated-live first-use guard: wrong/missing
// scratch binding fails closed, and every synthetic/forbidden operation throws
// instead of touching the daemon or falling back to fixtures.
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  FIRST_USE_LIVE_DAEMON,
  FIRST_USE_LIVE_READS,
  allowsFirstUseLiveRequest,
  assertFirstUseLiveDaemon,
  assertFirstUseLiveScratchEnv,
  invoke,
  normalizeDaemonUrl,
  verifyScratchBinding,
  verifyUpstreamScratchBinding,
} from "./first-use-live-core";
import { HANDLERS } from "../../src/web/live-invoke";

const SCRATCH = "/tmp/wenlan-preview-scratch/pages";

function jsonResponse(payload: unknown, status = 200): Response {
  return new Response(JSON.stringify(payload), { status });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("daemon gate", () => {
  it("accepts the exact scratch daemon, with or without trailing slash", () => {
    expect(assertFirstUseLiveDaemon(FIRST_USE_LIVE_DAEMON)).toBe(FIRST_USE_LIVE_DAEMON);
    expect(assertFirstUseLiveDaemon(`${FIRST_USE_LIVE_DAEMON}/`)).toBe(FIRST_USE_LIVE_DAEMON);
  });

  it("rejects absent, prod :7878, non-loopback, and wrong loopback ports", () => {
    expect(() => normalizeDaemonUrl(undefined)).toThrow("WENLAN_PREVIEW_DAEMON is required");
    expect(() => normalizeDaemonUrl("")).toThrow("WENLAN_PREVIEW_DAEMON is required");
    expect(() => normalizeDaemonUrl("http://127.0.0.1:7878")).toThrow(":7878");
    expect(() => normalizeDaemonUrl("http://192.168.1.20:17878")).toThrow("loopback");
    expect(() => normalizeDaemonUrl("http://example.com:17878")).toThrow("loopback");
    expect(() => normalizeDaemonUrl("https://127.0.0.1:17878")).toThrow("must be http");
    expect(() => normalizeDaemonUrl("not-a-url")).toThrow("not a valid URL");
    expect(() => assertFirstUseLiveDaemon("http://127.0.0.1:17879")).toThrow("exactly");
  });

  it("requires both scratch locations, absolute", () => {
    expect(() => assertFirstUseLiveScratchEnv({})).toThrow("WENLAN_PREVIEW_DATA_DIR is required");
    expect(() => assertFirstUseLiveScratchEnv({
      WENLAN_PREVIEW_DATA_DIR: "/tmp/scratch-data",
    })).toThrow("WENLAN_PREVIEW_KNOWLEDGE_PATH is required");
    expect(() => assertFirstUseLiveScratchEnv({
      WENLAN_PREVIEW_DATA_DIR: "relative/path",
      WENLAN_PREVIEW_KNOWLEDGE_PATH: SCRATCH,
    })).toThrow("must be absolute");
    expect(assertFirstUseLiveScratchEnv({
      WENLAN_PREVIEW_DATA_DIR: "/tmp/scratch-data",
      WENLAN_PREVIEW_KNOWLEDGE_PATH: `${SCRATCH}/`,
    })).toEqual({ dataDir: "/tmp/scratch-data", knowledgePath: SCRATCH });
  });
});

describe("HTTP policy", () => {
  it("allows reads and the one mutation path, blocks the rest", () => {
    expect(allowsFirstUseLiveRequest("GET", "/api/health")).toBe(true);
    expect(allowsFirstUseLiveRequest("GET", "/api/knowledge/path")).toBe(true);
    expect(allowsFirstUseLiveRequest("POST", "/api/import/memories")).toBe(true);
    expect(allowsFirstUseLiveRequest("POST", "/api/search")).toBe(true);
    expect(allowsFirstUseLiveRequest("POST", "/api/pages/search")).toBe(true);
    expect(allowsFirstUseLiveRequest("POST", "/api/config")).toBe(false);
    expect(allowsFirstUseLiveRequest("POST", "/api/on-device-model/download")).toBe(false);
    expect(allowsFirstUseLiveRequest("PUT", "/api/config")).toBe(false);
    expect(allowsFirstUseLiveRequest("DELETE", "/api/pages/x")).toBe(false);
    expect(allowsFirstUseLiveRequest("GET", "/not-an-api")).toBe(false);
  });
});

describe("scratch binding check", () => {
  const health = { status: "ok", db_initialized: true, version: "0.14.1" };
  it("passes on healthy daemon plus matching knowledge path", () => {
    expect(() => verifyScratchBinding({
      health,
      knowledgePath: `${SCRATCH}/`,
      expectedKnowledgePath: SCRATCH,
    })).not.toThrow();
  });
  it("fails closed on wrong path, missing path, or unhealthy daemon", () => {
    expect(() => verifyScratchBinding({
      health,
      knowledgePath: "/Users/someone/.wenlan/pages",
      expectedKnowledgePath: SCRATCH,
    })).toThrow("does not match scratch");
    expect(() => verifyScratchBinding({
      health,
      knowledgePath: null,
      expectedKnowledgePath: SCRATCH,
    })).toThrow("no knowledge path");
    expect(() => verifyScratchBinding({
      health: { status: "starting" },
      knowledgePath: SCRATCH,
      expectedKnowledgePath: SCRATCH,
    })).toThrow("not healthy");
  });
});

describe("invoke boundary", () => {
  it("every allowed read resolves to a real live handler", () => {
    for (const command of FIRST_USE_LIVE_READS) {
      expect(HANDLERS[command], command).toBeTypeOf("function");
    }
  });

  it("throws for synthetic, mutating, and unknown operations without network", async () => {
    const fetch = vi.fn(() => Promise.reject(new Error("must stay offline")));
    vi.stubGlobal("fetch", fetch);
    for (const command of [
      "store_memory", // setup probe write
      "download_on_device_model", // model download
      "on_device_model_download_bytes",
      "write_mcp_config", // client connection
      "install_client_plugin",
      "create_page", // authored page writes
      "create_page_draft",
      "update_page",
      "publish_page_draft",
      "discard_page_draft",
      "delete_page",
      "add_source", // source registration
      "sync_registered_source",
      "remove_source",
      "acknowledge_onboarding_milestone", // out-of-scope writes
      "accept_pending_revision",
      "review_page",
      "confirm_memory",
      "delete_memory",
      "set_api_key",
      "set_external_llm",
      "update_space",
      "no_such_command",
    ]) {
      await expect(invoke(command, {}), command).rejects.toThrow("unsupported command");
    }
    expect(fetch).not.toHaveBeenCalled();
  });

  it("answers passive/native stubs locally without claiming success", async () => {
    const fetch = vi.fn(() => Promise.reject(new Error("must stay offline")));
    vi.stubGlobal("fetch", fetch);
    await expect(invoke("set_traffic_lights_visible", { visible: true })).resolves.toBeNull();
    await expect(invoke("page_review_supported")).resolves.toBe("platform_unsupported");
    await expect(invoke("get_api_key")).resolves.toBeNull();
    expect(fetch).not.toHaveBeenCalled();
  });

  it("proxies the import mutation only after the scratch proof", async () => {
    const calls: string[] = [];
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      calls.push(url);
      if (url.endsWith("/api/health")) return jsonResponse({ status: "ok", version: "0.14.1" });
      if (url.endsWith("/api/knowledge/path")) return jsonResponse({ path: SCRATCH });
      if (url.endsWith("/api/import/memories")) {
        return jsonResponse({ imported: 2, skipped: 0, breakdown: {}, entities_created: 0, observations_added: 0, relations_created: 0, batch_id: "b1" });
      }
      throw new Error(`unexpected ${url}`);
    });
    vi.stubGlobal("fetch", fetch);

    const result = await invoke("import_memories_cmd", {
      source: "other",
      content: "a\nb",
      label: null,
      batchId: "b1",
      chunkIndex: 0,
      chunkTotal: 1,
    }, { expectedKnowledgePath: SCRATCH }) as { imported: number };
    expect(result.imported).toBe(2);
    expect(calls).toContain("/daemon/api/import/memories");
  });

  it("blocks the import mutation when the daemon answers a foreign path", async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/api/health")) return jsonResponse({ status: "ok", version: "0.14.1" });
      if (url.endsWith("/api/knowledge/path")) return jsonResponse({ path: "/Users/someone/.wenlan/pages" });
      throw new Error(`must not reach ${url}`);
    });
    vi.stubGlobal("fetch", fetch);

    await expect(invoke("import_memories_cmd", { source: "other", content: "x" }, {
      expectedKnowledgePath: SCRATCH,
    })).rejects.toThrow("does not match scratch");
    expect(fetch).not.toHaveBeenCalledWith(
      "/daemon/api/import/memories",
      expect.anything(),
    );
  });

  it("proves the upstream scratch binding over absolute fetch (vite middleware gate)", async () => {
    const ok = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url === `${FIRST_USE_LIVE_DAEMON}/api/health`) {
        return jsonResponse({ status: "ok", version: "0.14.1" });
      }
      if (url === `${FIRST_USE_LIVE_DAEMON}/api/knowledge/path`) {
        return jsonResponse({ path: SCRATCH });
      }
      throw new Error(`unexpected ${url}`);
    });
    vi.stubGlobal("fetch", ok);
    await expect(verifyUpstreamScratchBinding(FIRST_USE_LIVE_DAEMON, SCRATCH)).resolves.toBeUndefined();
    expect(ok).toHaveBeenCalledWith(`${FIRST_USE_LIVE_DAEMON}/api/health`, expect.anything());
    expect(ok).toHaveBeenCalledWith(`${FIRST_USE_LIVE_DAEMON}/api/knowledge/path`, expect.anything());

    vi.stubGlobal("fetch", vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/api/health")) return jsonResponse({ status: "ok", version: "0.14.1" });
      if (url.endsWith("/api/knowledge/path")) return jsonResponse({ path: "/Users/someone/.wenlan/pages" });
      throw new Error(`must not reach ${url}`);
    }));
    await expect(verifyUpstreamScratchBinding(FIRST_USE_LIVE_DAEMON, SCRATCH))
      .rejects.toThrow("does not match scratch");

    vi.stubGlobal("fetch", vi.fn(() => Promise.reject(new Error("connection refused"))));
    await expect(verifyUpstreamScratchBinding(FIRST_USE_LIVE_DAEMON, SCRATCH))
      .rejects.toThrow("unreachable");
  });

  it("serves Home review-queue reads and routing live", async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/api/memory/pending-revisions?limit=10")) return jsonResponse({ revisions: [] });
      if (url.endsWith("/api/refinery/queue")) return jsonResponse({ proposals: [] });
      if (url.endsWith("/api/memory/unconfirmed?limit=10")) return jsonResponse([{ id: "c1" }]);
      if (url.endsWith("/api/config/routing")) return new Response("not found", { status: 404 });
      throw new Error(`unexpected ${url}`);
    });
    vi.stubGlobal("fetch", fetch);
    await expect(invoke("list_pending_revisions", { limit: 10 })).resolves.toEqual([]);
    await expect(invoke("list_refinements", { limit: 10 })).resolves.toEqual({ proposals: [] });
    await expect(invoke("list_unconfirmed_memories", { limit: 10 })).resolves.toEqual([{ id: "c1" }]);
    // A pre-routing daemon 404s: null (LEGACY), exactly like the native client.
    await expect(invoke("get_resolved_routing")).resolves.toBeNull();
  });

  it("maps provider reads to live daemon routes, not setup fixtures", async () => {
    const fetch = vi.fn(async (input: RequestInfo | URL) => {
      const url = String(input);
      if (url.endsWith("/api/config")) {
        return jsonResponse({ external_llm_endpoint: null, external_llm_model: null });
      }
      if (url.endsWith("/api/on-device-model")) {
        return jsonResponse({ loaded: null, selected: null, models: [] });
      }
      throw new Error(`unexpected ${url}`);
    });
    vi.stubGlobal("fetch", fetch);

    await expect(invoke("get_external_llm", {}, { expectedKnowledgePath: SCRATCH }))
      .resolves.toEqual([null, null]);
    await expect(invoke("get_on_device_model", {}, { expectedKnowledgePath: SCRATCH }))
      .resolves.toEqual({ loaded: null, selected: null, models: [] });
  });
});


it("acknowledges only the real first-page announcement behind the scratch gate", async () => {
  const fetchMock = vi.fn()
    .mockResolvedValueOnce(jsonResponse({ status: "ok", db_initialized: true }))
    .mockResolvedValueOnce(jsonResponse({ path: SCRATCH }))
    .mockResolvedValueOnce(jsonResponse({ ok: true }));
  vi.stubGlobal("fetch", fetchMock);
  await invoke("acknowledge_onboarding_milestone", { id: "first-concept" }, { expectedKnowledgePath: SCRATCH });
  expect(fetchMock).toHaveBeenLastCalledWith("/daemon/api/onboarding/milestones/first-concept/acknowledge", { method: "POST" });
  expect(allowsFirstUseLiveRequest("POST", "/api/onboarding/milestones/first-concept/acknowledge")).toBe(true);
  await expect(invoke("acknowledge_onboarding_milestone", { id: "intelligence-ready" }, { expectedKnowledgePath: SCRATCH })).rejects.toThrow("unsupported command");
  expect(allowsFirstUseLiveRequest("POST", "/api/onboarding/reset")).toBe(false);
});
