// SPDX-License-Identifier: AGPL-3.0-only
/**
 * The real boot ladder, pinned as arithmetic.
 *
 * `src/App.test.tsx` substitutes a short policy so its fail-closed case stays
 * a fast unit test; this file is where the production schedule is actually
 * checked, so the two together still cover what one slow end-to-end test used
 * to cover on its own.
 */
import { describe, expect, it, vi } from "vitest";

import {
  ATTEMPT_TIMEOUT_MS,
  BOOT_QUERY_RETRY,
  RUST_HEALTH_LOOP_BUDGET_MS,
  bootQueryBudgetMs,
  bootQueryRetryDelay,
  runBootGate,
  withBootAttemptTimeout,
} from "./bootRetryPolicy";

describe("boot retry policy", () => {
  it("outlasts the Rust health loop's own wait for the same daemon", () => {
    // app/src/lib.rs: 10 attempts * 5s, plus 200ms * 2^i backoff for i in
    // 0..=8. If the frontend gave up first it would show the first-run wizard
    // to a configured user while Rust was still waiting on that same daemon.
    expect(RUST_HEALTH_LOOP_BUDGET_MS).toBe(152_200);
    expect(bootQueryBudgetMs()).toBeGreaterThanOrEqual(RUST_HEALTH_LOOP_BUDGET_MS);
    expect(bootQueryBudgetMs()).toBeGreaterThanOrEqual(150_000);
  });

  it("computes the budget as attempts x timeout plus every delay", () => {
    const attempts = BOOT_QUERY_RETRY + 1;
    const delays = Array.from({ length: BOOT_QUERY_RETRY }, (_, i) =>
      bootQueryRetryDelay(i),
    );
    const delayTotal = delays.reduce((sum, delay) => sum + delay, 0);

    expect(bootQueryBudgetMs()).toBe(attempts * ATTEMPT_TIMEOUT_MS + delayTotal);
    // 21 * 5_000 = 105_000, plus 1_000 + 2_000 + 18 * 3_000 = 57_000.
    expect(delayTotal).toBe(57_000);
    expect(bootQueryBudgetMs()).toBe(162_000);
  });

  it("backs off exponentially and then polls at a steady 3s", () => {
    expect(bootQueryRetryDelay(0)).toBe(1_000);
    expect(bootQueryRetryDelay(1)).toBe(2_000);
    expect(bootQueryRetryDelay(2)).toBe(3_000);
    expect(bootQueryRetryDelay(19)).toBe(3_000);
  });

  it("shrinks with the retry count, so a test double cannot inflate it", () => {
    expect(bootQueryBudgetMs(1)).toBe(2 * ATTEMPT_TIMEOUT_MS + 1_000);
    expect(bootQueryBudgetMs(0)).toBe(ATTEMPT_TIMEOUT_MS);
  });

  it("times out a hung boot attempt instead of waiting forever", async () => {
    vi.useFakeTimers();
    try {
      const hung = withBootAttemptTimeout(new Promise<string>(() => {}), 5000);
      const timedOut = expect(hung).rejects.toThrow(
        "Boot attempt timed out after 5000 ms waiting for the app to answer",
      );
      await vi.advanceTimersByTimeAsync(5000);
      await timedOut;

      const fast = withBootAttemptTimeout(
        new Promise<string>((resolve) => {
          setTimeout(() => resolve("ok"), 10);
        }),
        5000,
      );
      const settled = expect(fast).resolves.toBe("ok");
      await vi.advanceTimersByTimeAsync(10);
      await settled;
      // The timer is cleared when the promise settles: running well past it
      // must not reject afterwards.
      await vi.advanceTimersByTimeAsync(10_000);
      await expect(fast).resolves.toBe("ok");
    } finally {
      vi.useRealTimers();
    }
  });

  it("returns a fast gate success without waiting", async () => {
    vi.useFakeTimers();
    try {
      const attempt = vi.fn<() => Promise<string>>().mockResolvedValue("ok");
      await expect(runBootGate(attempt, { retry: 2 })).resolves.toBe("ok");
      expect(attempt).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });

  it("retries a failing gate with the policy delays and rethrows the last error", async () => {
    vi.useFakeTimers();
    try {
      const attempt = vi
        .fn<() => Promise<string>>()
        .mockRejectedValueOnce(new Error("first"))
        .mockRejectedValueOnce(new Error("second"))
        .mockRejectedValue(new Error("last"));
      let settled: string | null = null;
      const assertion = runBootGate(attempt, { retry: 2 }).then(
        () => {
          settled = "resolved";
        },
        (error: unknown) => {
          settled = error instanceof Error ? error.message : String(error);
        },
      );
      // Two immediate failures cost only the real policy delays between them:
      // 1_000 after the first, 2_000 after the second.
      await vi.advanceTimersByTimeAsync(2_999);
      expect(attempt).toHaveBeenCalledTimes(2);
      expect(settled).toBeNull();
      await vi.advanceTimersByTimeAsync(1);
      await assertion;
      expect(attempt).toHaveBeenCalledTimes(3);
      expect(settled).toBe("last");
    } finally {
      vi.useRealTimers();
    }
  });

  it("bounds a hung gate by the budget instead of waiting forever", async () => {
    vi.useFakeTimers();
    try {
      const retry = 2;
      const budget = bootQueryBudgetMs(retry);
      let settled: string | null = null;
      const assertion = runBootGate(() => new Promise<string>(() => {}), {
        retry,
      }).then(
        () => {
          settled = "resolved";
        },
        (error: unknown) => {
          settled = error instanceof Error ? error.message : String(error);
        },
      );
      await vi.advanceTimersByTimeAsync(budget - 1);
      expect(settled).toBeNull();
      await vi.advanceTimersByTimeAsync(1);
      await assertion;
      expect(settled).toBe(
        "Boot attempt timed out after 5000 ms waiting for the app to answer",
      );
    } finally {
      vi.useRealTimers();
    }
  });

  it("stops the retry sleep when the gate signal aborts", async () => {
    vi.useFakeTimers();
    try {
      const controller = new AbortController();
      const attempt = vi.fn<() => Promise<string>>().mockRejectedValue(new Error("down"));
      let settled: string | null = null;
      const assertion = runBootGate(attempt, { retry: 2, signal: controller.signal }).then(
        () => {
          settled = "resolved";
        },
        (error: unknown) => {
          settled = error instanceof Error ? error.message : String(error);
        },
      );
      await vi.advanceTimersByTimeAsync(500);
      expect(attempt).toHaveBeenCalledTimes(1);
      expect(settled).toBeNull();
      controller.abort();
      await assertion;
      expect(settled).toBe("down");
      expect(attempt).toHaveBeenCalledTimes(1);
      // The cleared delay must not fire a late second attempt.
      await vi.advanceTimersByTimeAsync(10_000);
      expect(attempt).toHaveBeenCalledTimes(1);
    } finally {
      vi.useRealTimers();
    }
  });
});
