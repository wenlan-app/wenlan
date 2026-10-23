// SPDX-License-Identifier: AGPL-3.0-only
/**
 * The retry schedule for the first-run gate query in `src/App.tsx`.
 *
 * This lives in its own module for one reason: the total budget is a contract
 * with the Rust side, and a contract nobody can read is a contract nobody
 * keeps. `bootRetryPolicy.test.ts` pins the arithmetic.
 *
 * ## Why the budget is what it is
 *
 * The daemon binds its port BEFORE it can serve: migrations, embedder load and
 * projection reconcile all happen behind an already-bound socket
 * (`crates/wenlan-server/src/main.rs` binds first, `axum::serve` starts in
 * `crates/wenlan-server/src/main/runtime.rs`, and the mute-port window is
 * described in `crates/wenlan-server/src/main/startup.rs`). So "connection
 * accepted" does not mean "daemon ready", and the gate has to outlast that
 * window rather than treat it as a failure.
 *
 * The app already has an answer for how long that window can be: the Rust
 * health loop in `app/src/lib.rs` waits ~152s for the same daemon
 * (10 attempts, each bounded by the 5s HEALTH_TIMEOUT, with 200ms * 2^i
 * backoff between them — 50_000ms of attempts + 102_200ms of delays). This
 * frontend gate must not give up before the Rust side does, or it drops an
 * already-configured user into the fail-closed SetupWizard while the app's own
 * startup path is still patiently waiting for the very same daemon.
 *
 * So: the budget below is >= RUST_HEALTH_LOOP_BUDGET_MS, on purpose.
 */

/**
 * Per-attempt ceiling, mirroring `HEALTH_TIMEOUT` in `app/src/api.rs`.
 *
 * `get_setup_status` attaches that timeout explicitly, so a bound-but-mute
 * daemon costs one attempt this much and no more.
 */
export const ATTEMPT_TIMEOUT_MS = 5_000;

/**
 * What the Rust health loop in `app/src/lib.rs` tolerates for the same daemon:
 * 10 attempts * 5_000ms, plus 200ms * 2^i backoff for i in 0..=8
 * (200 * 511 = 102_200ms). The frontend budget must be at least this.
 */
export const RUST_HEALTH_LOOP_BUDGET_MS = 10 * ATTEMPT_TIMEOUT_MS + 200 * 511;

/** Retries AFTER the first attempt. */
export const BOOT_QUERY_RETRY = 20;

/** Exponential backoff, capped so late attempts still poll every 3s. */
export function bootQueryRetryDelay(attemptIndex: number): number {
  return Math.min(1000 * 2 ** attemptIndex, 3000);
}

/**
 * Worst-case wall time before the gate gives up: every attempt burning its
 * full timeout, plus every delay between them.
 */
export function bootQueryBudgetMs(retry: number = BOOT_QUERY_RETRY): number {
  let total = (retry + 1) * ATTEMPT_TIMEOUT_MS;
  for (let attemptIndex = 0; attemptIndex < retry; attemptIndex += 1) {
    total += bootQueryRetryDelay(attemptIndex);
  }
  return total;
}

/**
 * How long "Starting Wenlan" may stay a single unexplained line.
 *
 * The gate above can legitimately hold this screen for the better part of
 * three minutes, and for all of it the user is looking at one sentence with
 * no subject: they cannot tell a slow cold start from a machine that will
 * never finish booting. After this much, the screen names what it is waiting
 * for. Well inside `bootQueryBudgetMs()`, on purpose — this is a caption on a
 * wait that is still healthy, not a deadline.
 */
export const BOOT_SLOW_NOTICE_MS = 15_000;

/**
 * After the gate gives up on a user who opened early, the app keeps asking
 * the daemon this often, one bounded attempt at a time. A daemon that comes up
 * later on its own (launchd, a CLI restart, a slow cold boot) then clears the
 * sidebar warning without the user pressing anything.
 */
export const FAILED_GATE_RECHECK_MS = 5_000;

/**
 * Bounds one gate attempt: a hung IPC call must cost one attempt, not the
 * whole gate. React Query only retries after the promise settles, so without
 * this a `should_show_wizard` that never answers holds the boot gate pending
 * forever. The timer is cleared as soon as the promise settles, so a fast
 * answer never trips a late rejection.
 */
export function withBootAttemptTimeout<T>(
  promise: Promise<T>,
  ms: number = ATTEMPT_TIMEOUT_MS,
): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<T>((_, reject) => {
    timer = setTimeout(() => {
      reject(new Error(`Boot attempt timed out after ${ms} ms waiting for the app to answer`));
    }, ms);
  });
  return Promise.race([promise, timeout]).finally(() => {
    if (timer !== undefined) clearTimeout(timer);
  });
}

/** Overrides for `runBootGate`; every field falls back to the policy default. */
export interface BootGateOptions {
  /** Retries AFTER the first attempt. Default `BOOT_QUERY_RETRY`. */
  retry?: number;
  /** Wait between attempts, by zero-based attempt index. Default `bootQueryRetryDelay`. */
  delay?: (attemptIndex: number) => number;
  /** Per-attempt ceiling in ms. Default `ATTEMPT_TIMEOUT_MS`. */
  attemptTimeoutMs?: number;
  /** Stops the waits between attempts. Default none. */
  signal?: AbortSignal;
}

/**
 * One `setTimeout` wait that ends early when `signal` aborts. The timer is
 * cleared on abort so a cancelled gate holds no timer open.
 */
function abortableBootGateDelay(ms: number, signal?: AbortSignal): Promise<void> {
  return new Promise<void>((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason ?? new DOMException("AbortError"));
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    const onAbort = () => {
      clearTimeout(timer);
      reject(signal?.reason ?? new DOMException("AbortError"));
    };
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/**
 * Runs one boot gate attempt function to settlement without asking React
 * Query to retry it. React Query pauses its own retries while
 * `document.visibilityState` is `"hidden"`, and a hidden or occluded webview
 * reports hidden, so a gate that relies on query retries parks until the
 * window is visible again instead of failing closed on its own. This loop
 * retries with a plain `setTimeout` delay and never consults `document`,
 * `focusManager`, or `navigator.onLine`.
 *
 * With the default timeout and delay, the worst case equals
 * `bootQueryBudgetMs(retry)` for the same retry count: every attempt burns
 * its full timeout, plus every delay between attempts. Passing `signal`
 * (React Query cancels the query when it unmounts) ends a pending delay at
 * once and rethrows the last attempt error instead of sleeping on.
 */
export async function runBootGate<T>(
  attempt: () => Promise<T>,
  options?: BootGateOptions,
): Promise<T> {
  const retry = options?.retry ?? BOOT_QUERY_RETRY;
  const delay = options?.delay ?? bootQueryRetryDelay;
  const attemptTimeoutMs = options?.attemptTimeoutMs ?? ATTEMPT_TIMEOUT_MS;
  const signal = options?.signal;
  let attemptIndex = 0;
  for (;;) {
    try {
      return await withBootAttemptTimeout(attempt(), attemptTimeoutMs);
    } catch (error) {
      if (attemptIndex >= retry) throw error;
      try {
        await abortableBootGateDelay(delay(attemptIndex), signal);
      } catch {
        throw error;
      }
      attemptIndex += 1;
    }
  }
}
