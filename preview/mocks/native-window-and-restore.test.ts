// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke as fixtureInvoke } from "./core";
import { liveInvoke } from "./live-invoke";
import { invoke as devInvoke } from "./dev-live-core";
import { invoke as firstUseInvoke } from "./first-use-live-core";
import { invoke as reviewInvoke, resetReviewRuntime } from "../../review/tauri-core";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
import { restoreFixturePage } from "../../e2e/tauriMock/pageRestore";
import { createReviewDecisionFixture } from "../../e2e/fixtures/reviewDecisions";
import { PAGES, PRISTINE } from "../fixtures";

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

describe("native-only composition command", () => {
  beforeEach(() => {
    (window as { __PREVIEW_FIXTURES__?: boolean }).__PREVIEW_FIXTURES__ = true;
    resetReviewRuntime();
  });
  it.each([
    ["fixture", fixtureInvoke], ["live", liveInvoke], ["dev readonly", devInvoke],
    ["first use", firstUseInvoke], ["Review", reviewInvoke],
    ["Tauri runtime", (command: string, args: Record<string, unknown>) =>
      new TauriMockRuntime(createReviewDecisionFixture("wiki-folders")).invoke(command, args)],
  ] as const)("%s consumes both states without network or warnings", async (_name, invoke) => {
    const fetch = vi.fn(() => { throw new Error("Unexpected network access"); });
    vi.stubGlobal("fetch", fetch);
    const warn = vi.spyOn(console, "warn");
    await expect(invoke("set_editor_composing", { active: true })).resolves.toBeNull();
    await expect(invoke("set_editor_composing", { active: false })).resolves.toBeNull();
    expect(fetch).not.toHaveBeenCalled();
    expect(warn).not.toHaveBeenCalled();
  });
});

describe("restore Page fixture and live contracts", () => {
  const archived = () => ({ ...structuredClone(PRISTINE["page-cited"]), status: "archived", creation_kind: "authored", version: 4 });

  it("restores the same fixture identity and body, fences stale versions, and permits an exact active retry", async () => {
    const before = archived();
    const fixture = createReviewDecisionFixture("wiki-folders");
    const runtime = new TauriMockRuntime({ ...fixture, pages: [before] });
    const request = { id: before.id, expectedVersion: 4 };
    const restored = await runtime.invoke("restore_page", request);
    expect(restored).toEqual({ id: before.id, title: before.title, version: 5, status: "active", projection_status: "not_configured" });
    const after = await runtime.invoke("get_page", { id: before.id });
    expect(after).toMatchObject({ id: before.id, content: before.content, source_memory_ids: before.source_memory_ids, status: "active", version: 5 });
    await expect(runtime.invoke("restore_page", request)).rejects.toThrow("version conflict");
    await expect(runtime.invoke("restore_page", { ...request, expectedVersion: 5 })).resolves.toEqual(restored);
  });

  it("keeps preview fixture restores local", async () => {
    const before = archived();
    PAGES[before.id] = before;
    (window as { __PREVIEW_FIXTURES__?: boolean }).__PREVIEW_FIXTURES__ = true;
    const fetch = vi.fn(() => { throw new Error("Unexpected network access"); });
    vi.stubGlobal("fetch", fetch);
    await expect(fixtureInvoke("restore_page", { id: before.id, expectedVersion: 4, title: "Recovered title" }))
      .resolves.toMatchObject({ id: before.id, title: "Recovered title", version: 5, status: "active" });
    expect(PAGES[before.id].content).toBe(before.content);
    expect(fetch).not.toHaveBeenCalled();
    PAGES[before.id] = structuredClone(PRISTINE["page-cited"]);
  });

  it("rejects title collisions without mutating the archived row and allows a new ordinary title", () => {
    const before = archived();
    const other = { ...before, id: "other", status: "active", title: before.title.toUpperCase() };
    expect(() => restoreFixturePage(before, [before, other], 4)).toThrow("title already exists");
    expect(before).toMatchObject({ status: "archived", version: 4 });
    expect(restoreFixturePage(before, [before, other], 4, "Recovered").response).toMatchObject({ title: "Recovered", version: 5 });
  });

  it.each(["entity", "source", "imported"])("preserves %s eligibility/title restrictions", (kind) => {
    const page = { ...archived(), creation_kind: kind };
    expect(() => restoreFixturePage(page, [page], 4, "Changed")).toThrow(kind === "entity" ? "ordinary pages" : "retain their original title");
    if (kind !== "entity") expect(restoreFixturePage(page, [page], 4).response.title).toBe(page.title);
  });

  it("rejects draft, stale, missing and invalid-title requests", () => {
    const page = archived();
    expect(() => restoreFixturePage({ ...page, status: "draft" }, [], 4)).toThrow("ordinary pages");
    expect(() => restoreFixturePage(page, [], 3)).toThrow("version conflict");
    expect(() => restoreFixturePage(undefined, [], 4)).toThrow("not found");
    expect(() => restoreFixturePage(page, [], 0)).toThrow("expected_version");
    expect(() => restoreFixturePage(page, [], 4, "line\nbreak")).toThrow("single line");
  });

  it("sends the typed restore POST with explicit-browse headers and preserves projection failures", async () => {
    const response = { id: "a/b", title: "Recovered", version: 5, status: "active", projection_status: "pending", projection_error: "read-only projection" };
    const fetch = vi.fn(async () => new Response(JSON.stringify(response)));
    vi.stubGlobal("fetch", fetch);
    await expect(liveInvoke("restore_page", { id: "a/b", expectedVersion: 4, title: "Recovered" })).resolves.toEqual(response);
    expect(fetch).toHaveBeenCalledWith("/daemon/api/pages/a%2Fb/restore", expect.objectContaining({
      method: "POST", body: JSON.stringify({ expected_version: 4, title: "Recovered" }),
      headers: expect.objectContaining({ "x-wenlan-reader-intent": "explicit", "x-wenlan-truth-contract": "1" }),
    }));
    fetch.mockResolvedValueOnce(new Response("page version conflict", { status: 409 }));
    await expect(liveInvoke("restore_page", { id: "a/b", expectedVersion: 4 })).rejects.toThrow();
  });

  it("does not grant restores to readonly dev or first-use preview", async () => {
    const fetch = vi.fn(() => { throw new Error("Unexpected network access"); });
    vi.stubGlobal("fetch", fetch);
    await expect(devInvoke("restore_page", { id: "page-a", expectedVersion: 1 })).rejects.toThrow("read-only");
    await expect(firstUseInvoke("restore_page", { id: "page-a", expectedVersion: 1 })).rejects.toThrow();
    expect(fetch).not.toHaveBeenCalled();
  });
});
