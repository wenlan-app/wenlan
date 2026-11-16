// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { renderHook, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";

const mocks = vi.hoisted(() => ({
  shouldShowWizard: vi.fn(),
  getResolvedRouting: vi.fn(),
  getBackgroundAiEnabled: vi.fn(),
  setSourcePin: vi.fn(),
}));
vi.mock("./tauri", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./tauri")>();
  return { ...actual, ...mocks };
});

import {
  fillUnsetPinsAtLaunch,
  resetLaunchPinFillForTest,
  useLaunchPinFill,
} from "./launchPinFill";
import type { ResolvedRouting } from "./tauri";

/** Routing with a loaded on-device model and both jobs unpinned. */
function unpinnedWithLoadedModel(): ResolvedRouting {
  return {
    everyday: { source: "basic", model: null, mode: "unconfigured", pin: null },
    synthesis: { source: "none", model: null, mode: "unconfigured", pin: null },
    pool: {
      anthropic: { configured: false, everyday_model: null, synthesis_model: null },
      external: null,
      on_device: { selected: "qwen3-4b", loaded: true },
    },
  };
}

beforeEach(() => {
  resetLaunchPinFillForTest();
  mocks.shouldShowWizard.mockResolvedValue(false);
  mocks.getResolvedRouting.mockResolvedValue(unpinnedWithLoadedModel());
  mocks.getBackgroundAiEnabled.mockResolvedValue(true);
  mocks.setSourcePin.mockResolvedValue(undefined);
});

afterEach(() => {
  Object.values(mocks).forEach((m) => m.mockReset());
});

describe("fillUnsetPinsAtLaunch", () => {
  it("does not read desktop setup state in the web build", async () => {
    vi.stubGlobal("__WENLAN_WEB__", true);
    await expect(fillUnsetPinsAtLaunch()).resolves.toBeNull();
    expect(mocks.shouldShowWizard).not.toHaveBeenCalled();
    expect(mocks.getResolvedRouting).not.toHaveBeenCalled();
    vi.unstubAllGlobals();
  });

  it("pins the loaded model for both jobs when setup is complete", async () => {
    await expect(fillUnsetPinsAtLaunch()).resolves.toEqual({
      written: { everyday: "on_device", synthesis: "on_device" },
      inEffect: { everyday: "on_device", synthesis: "on_device" },
    });
    expect(mocks.setSourcePin).toHaveBeenCalledWith("on_device", "on_device", true);
  });

  it("runs once per launch, however many times it is called", async () => {
    await fillUnsetPinsAtLaunch();
    await fillUnsetPinsAtLaunch();
    expect(mocks.getResolvedRouting).toHaveBeenCalledTimes(1);
    expect(mocks.setSourcePin).toHaveBeenCalledTimes(1);
  });

  it("does not let two calls in the same tick both fill", async () => {
    // React's development double-effect mounts the hook twice immediately.
    const [first, second] = await Promise.all([
      fillUnsetPinsAtLaunch(),
      fillUnsetPinsAtLaunch(),
    ]);
    expect(mocks.setSourcePin).toHaveBeenCalledTimes(1);
    expect([first, second].filter((r) => r !== null)).toHaveLength(1);
  });

  it("writes nothing while the wizard would be showing, and can still fill afterwards", async () => {
    mocks.shouldShowWizard.mockResolvedValueOnce(true);
    await expect(fillUnsetPinsAtLaunch()).resolves.toBeNull();
    expect(mocks.getResolvedRouting).not.toHaveBeenCalled();
    expect(mocks.setSourcePin).not.toHaveBeenCalled();

    // Finishing setup and reaching the main screen must not be blocked by the
    // attempt that bailed out.
    await fillUnsetPinsAtLaunch();
    expect(mocks.setSourcePin).toHaveBeenCalledTimes(1);
  });

  it("writes nothing when no source is configured yet", async () => {
    const routing = unpinnedWithLoadedModel();
    routing.pool.on_device = { selected: "qwen3-4b", loaded: false };
    mocks.getResolvedRouting.mockResolvedValue(routing);
    await expect(fillUnsetPinsAtLaunch()).resolves.toEqual({
      written: { everyday: null, synthesis: null },
      inEffect: { everyday: null, synthesis: null },
    });
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
  });

  it("leaves a job the user already pinned alone", async () => {
    const routing = unpinnedWithLoadedModel();
    routing.everyday = { source: "external", model: "llama3", mode: "pinned", pin: "external" };
    mocks.getResolvedRouting.mockResolvedValue(routing);
    await fillUnsetPinsAtLaunch();
    expect(mocks.setSourcePin).toHaveBeenCalledWith(null, "on_device", true);
  });

  it("logs and gives up when the daemon cannot say whether setup is done", async () => {
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.shouldShowWizard.mockRejectedValueOnce(new Error("connection refused"));
    await expect(fillUnsetPinsAtLaunch()).resolves.toBeNull();
    expect(error).toHaveBeenCalled();
    expect(mocks.setSourcePin).not.toHaveBeenCalled();
    error.mockRestore();
  });

  it("does not surface a failed pin write, and stays open to a later attempt", async () => {
    vi.useFakeTimers();
    const error = vi.spyOn(console, "error").mockImplementation(() => {});
    mocks.setSourcePin.mockRejectedValue(new Error("daemon down"));
    const filled = fillUnsetPinsAtLaunch();
    await vi.advanceTimersByTimeAsync(10_000);
    await expect(filled).resolves.toBeNull();
    expect(mocks.setSourcePin).toHaveBeenCalledTimes(3);
    error.mockRestore();
    vi.useRealTimers();

    // Giving up must not disable the fill for the rest of the session. A daemon
    // that was down for those few seconds is the exact case this exists for, so
    // the next mount has to be allowed to try again.
    mocks.setSourcePin.mockReset();
    mocks.setSourcePin.mockResolvedValue(undefined);
    await expect(fillUnsetPinsAtLaunch()).resolves.toEqual({
      written: { everyday: "on_device", synthesis: "on_device" },
      inEffect: { everyday: "on_device", synthesis: "on_device" },
    });
    expect(mocks.setSourcePin).toHaveBeenCalledTimes(1);
  });

  it("stays open to a later attempt on a daemon with no routing endpoint", async () => {
    mocks.getResolvedRouting.mockResolvedValueOnce(null);
    await expect(fillUnsetPinsAtLaunch()).resolves.toBeNull();
    await fillUnsetPinsAtLaunch();
    expect(mocks.setSourcePin).toHaveBeenCalledWith("on_device", "on_device", true);
  });
});

describe("useLaunchPinFill", () => {
  /** Mounts the hook and returns a spy on the query invalidation it should do. */
  function mountHook() {
    const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const invalidate = vi.spyOn(queryClient, "invalidateQueries");
    const wrapper = ({ children }: { children: ReactNode }) =>
      createElement(QueryClientProvider, { client: queryClient }, children);
    renderHook(() => useLaunchPinFill(), { wrapper });
    return invalidate;
  }

  /** Lets the fill's promise chain finish under real timers. */
  async function settle() {
    for (let i = 0; i < 3; i++) await new Promise((resolve) => setTimeout(resolve, 0));
  }

  it("refetches routing after it writes a pin", async () => {
    // Home reads routing at mount, before this write lands. Without the
    // refetch it would keep telling the user to choose a model they now have.
    const invalidate = mountHook();
    await waitFor(() => expect(mocks.setSourcePin).toHaveBeenCalled());
    await waitFor(() =>
      expect(invalidate).toHaveBeenCalledWith({ queryKey: ["resolvedRouting"] }),
    );
  });

  it("does not refetch when both jobs were already pinned", async () => {
    const routing = unpinnedWithLoadedModel();
    routing.everyday = { source: "external", model: "llama3", mode: "pinned", pin: "external" };
    routing.synthesis = { source: "external", model: "llama3", mode: "pinned", pin: "external" };
    mocks.getResolvedRouting.mockResolvedValue(routing);

    const invalidate = mountHook();
    await waitFor(() => expect(mocks.getResolvedRouting).toHaveBeenCalled());
    await settle();

    expect(mocks.setSourcePin).not.toHaveBeenCalled();
    // Nothing was written, so nothing in the cache went stale.
    expect(invalidate).not.toHaveBeenCalled();
  });

  it("does not refetch on a daemon with no routing endpoint", async () => {
    mocks.getResolvedRouting.mockResolvedValue(null);
    const invalidate = mountHook();
    await waitFor(() => expect(mocks.getResolvedRouting).toHaveBeenCalled());
    await settle();
    expect(invalidate).not.toHaveBeenCalled();
  });
});
