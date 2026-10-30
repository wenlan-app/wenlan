// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, describe, expect, it, vi } from "vitest";
import { invoke as fixtureInvoke } from "./core";
import { liveInvoke } from "./live-invoke";
import { invoke as devInvoke } from "./dev-live-core";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
import { restoreFixturePage } from "../../e2e/tauriMock/pageRestore";
import { createReviewDecisionFixture } from "../../e2e/fixtures/reviewDecisions";
import { PAGES, PRISTINE } from "../fixtures";
import { parseRestorePageError } from "../../src/lib/tauri";

afterEach(() => { vi.unstubAllGlobals(); vi.restoreAllMocks(); });

const archived = () => ({ ...structuredClone(PRISTINE["page-cited"]), status: "archived", creation_kind: "authored", version: 4 });

function refusedWith(run: () => unknown) {
  try { run(); } catch (error) { return parseRestorePageError(error); }
  throw new Error("expected a refusal");
}

describe("restore_page fixtures", () => {
  it("restores the same id and body in the Tauri mock, fences stale versions, and replays an exact retry", async () => {
    const before = archived();
    const runtime = new TauriMockRuntime({ ...createReviewDecisionFixture("wiki-folders"), pages: [before] });
    const request = { id: before.id, expectedVersion: 4 };
    const restored = await runtime.invoke("restore_page", request);
    expect(restored).toEqual({ id: before.id, title: before.title, version: 5, status: "active", projection_status: "not_configured" });
    expect(await runtime.invoke("get_page", { id: before.id })).toMatchObject({ id: before.id, content: before.content, status: "active", version: 5 });
    const stale = await runtime.invoke("restore_page", request).catch((error: unknown) => parseRestorePageError(error));
    expect(stale).toMatchObject({ code: "page_version_conflict", currentVersion: 5 });
    await expect(runtime.invoke("restore_page", { ...request, expectedVersion: 5 })).resolves.toEqual(restored);
  });

  it("refuses with the daemon's typed codes", () => {
    const page = archived();
    const other = { ...page, id: "other", status: "active", title: page.title.toUpperCase() };
    expect(refusedWith(() => restoreFixturePage(page, [page, other], 4))).toMatchObject({ code: "page_title_taken" });
    expect(page).toMatchObject({ status: "archived", version: 4 });
    expect(restoreFixturePage(page, [page, other], 4, "Recovered").response).toMatchObject({ title: "Recovered", version: 5 });
    expect(refusedWith(() => restoreFixturePage({ ...page, creation_kind: "source" }, [], 4, "Changed"))).toMatchObject({ code: "page_title_fixed" });
    expect(refusedWith(() => restoreFixturePage({ ...page, creation_kind: "entity" }, [], 4))).toMatchObject({ code: "page_not_restorable" });
    expect(refusedWith(() => restoreFixturePage({ ...page, status: "draft" }, [], 4))).toMatchObject({ code: "page_not_restorable" });
    expect(refusedWith(() => restoreFixturePage({ ...page, status: "active" }, [], 4, "Another"))).toMatchObject({ code: "page_not_archived", currentVersion: 4 });
    expect(refusedWith(() => restoreFixturePage(page, [], 3))).toMatchObject({ code: "page_version_conflict", currentVersion: 4 });
    expect(() => restoreFixturePage(undefined, [], 4)).toThrow("not found");
    expect(() => restoreFixturePage(page, [], 4, "line\nbreak")).toThrow("single line");
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

  it("sends the live restore POST with explicit-browse headers and keeps the typed refusal", async () => {
    const response = { id: "a/b", title: "Recovered", version: 5, status: "active", projection_status: "pending", projection_error: "read-only projection" };
    const fetch = vi.fn(async () => new Response(JSON.stringify(response)));
    vi.stubGlobal("fetch", fetch);
    await expect(liveInvoke("restore_page", { id: "a/b", expectedVersion: 4, title: "Recovered" })).resolves.toEqual(response);
    expect(fetch).toHaveBeenCalledWith("/daemon/api/pages/a%2Fb/restore", expect.objectContaining({
      method: "POST", body: JSON.stringify({ expected_version: 4, title: "Recovered" }),
      headers: expect.objectContaining({ "x-wenlan-reader-intent": "explicit", "x-wenlan-truth-contract": "1" }),
    }));
    fetch.mockResolvedValueOnce(new Response(JSON.stringify({ code: "page_title_taken", error: "page title already exists" }), { status: 409 }));
    const refused = await liveInvoke("restore_page", { id: "a/b", expectedVersion: 4 }).catch((error: unknown) => parseRestorePageError(error));
    expect(refused).toMatchObject({ code: "page_title_taken" });
  });

  it("does not grant restores to the read-only dev preview", async () => {
    const fetch = vi.fn(() => { throw new Error("Unexpected network access"); });
    vi.stubGlobal("fetch", fetch);
    await expect(devInvoke("restore_page", { id: "page-a", expectedVersion: 1 })).rejects.toThrow();
    expect(fetch).not.toHaveBeenCalled();
  });
});
