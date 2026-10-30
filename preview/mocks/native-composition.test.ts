// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { invoke as fixtureInvoke } from "./core";
import { liveInvoke } from "./live-invoke";
import { invoke as devInvoke } from "./dev-live-core";
import { invoke as firstUseInvoke } from "./first-use-live-core";
import { invoke as reviewInvoke, resetReviewRuntime } from "../../review/tauri-core";
import { TauriMockRuntime } from "../../e2e/tauriMock/runtime";
import { createReviewDecisionFixture } from "../../e2e/fixtures/reviewDecisions";

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
