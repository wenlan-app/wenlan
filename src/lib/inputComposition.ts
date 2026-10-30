// SPDX-License-Identifier: AGPL-3.0-only
// One document-wide record of whether an IME composition (Pinyin, Bopomofo,
// Japanese, Korean...) is in progress, so navigation, global hotkeys and the
// quit flush can wait instead of interrupting the candidate the user is
// choosing. The editors keep their own per-field state; this is the shared view.

type Reporter = (active: boolean) => void;
type Guard = {
  active: EventTarget | null;
  reporters: Set<Reporter>;
  endWaiters: Set<() => void>;
  lastReportAt: number;
  cleanup: () => void;
  reset: (reason: string | null) => void;
};
const guards = new WeakMap<Document, Guard>();

/**
 * A composition that has produced no IME event for this long is treated as
 * lost (its compositionend never arrived), so it cannot keep shortcuts blocked
 * for the rest of the session. The native guard in `app/src/global_shortcuts.rs`
 * uses the same bound.
 */
export const COMPOSITION_IDLE_RESET_MS = 30_000;
/**
 * While the user keeps typing into one long composition (Bopomofo can hold a
 * whole phrase), re-report it this often so the native idle bound does not
 * expire a composition that is still alive.
 */
export const COMPOSITION_HEARTBEAT_MS = 10_000;

/** Observe the browser's IME lifecycle for application navigation, including native inputs. */
export function observeInputComposition(report: Reporter, doc: Document = document): () => void {
  let guard = guards.get(doc);
  if (!guard) {
    let idleTimer: ReturnType<typeof setTimeout> | null = null;
    const current: Guard = {
      active: null,
      reporters: new Set(),
      endWaiters: new Set(),
      lastReportAt: 0,
      cleanup: () => {},
      reset: () => {},
    };
    const notify = (active: boolean) => {
      current.lastReportAt = Date.now();
      for (const listener of current.reporters) listener(active);
    };
    const armIdle = () => {
      if (idleTimer !== null) clearTimeout(idleTimer);
      idleTimer = setTimeout(() => {
        idleTimer = null;
        current.reset("no IME event for 30 s");
      }, COMPOSITION_IDLE_RESET_MS);
    };
    const publish = (target: EventTarget | null) => {
      const previous = current.active !== null;
      current.active = target;
      if (target === null) {
        removals.disconnect();
        if (idleTimer !== null) clearTimeout(idleTimer);
        idleTimer = null;
        const waiters = [...current.endWaiters];
        current.endWaiters.clear();
        for (const resolve of waiters) resolve();
      } else {
        armIdle();
      }
      if (previous !== (target !== null)) notify(target !== null);
    };
    // A replaced editor may be detached without receiving compositionend.
    // Observe removals only while composing, so a stale native bit cannot block shortcuts.
    const removals = new MutationObserver(() => {
      if (current.active instanceof Node && !current.active.isConnected) publish(null);
    });
    const begin = (target: EventTarget | null) => {
      publish(target);
      removals.observe(doc, { childList: true, subtree: true });
    };
    const start = (event: CompositionEvent) => begin(event.target);
    const update = (event: CompositionEvent) => {
      // A composition that outlived a reset (window blur, idle bound) is live
      // again as soon as it produces text; block shortcuts again.
      if (current.active === null) {
        begin(event.target);
        return;
      }
      current.active = event.target;
      armIdle();
      if (Date.now() - current.lastReportAt >= COMPOSITION_HEARTBEAT_MS) notify(true);
    };
    const end = (event: CompositionEvent) => {
      if (current.active !== event.target) return;
      publish(null);
    };
    current.reset = (reason) => {
      if (current.active === null) return;
      if (reason) console.warn(`[input-composition] released a composition without compositionend: ${reason}`);
      publish(null);
    };
    // Chromium (WebView2) and WebKit finish the composition when the webview
    // loses focus. If that compositionend was lost, nothing else would clear the
    // record, and global hotkeys would stay blocked (Alt+Tab mid-composition).
    const blur = () => current.reset(null);
    const pagehide = () => current.reset(null);
    doc.addEventListener("compositionstart", start, true);
    doc.addEventListener("compositionupdate", update, true);
    doc.addEventListener("compositionend", end, true);
    doc.defaultView?.addEventListener("blur", blur);
    doc.defaultView?.addEventListener("pagehide", pagehide);
    current.cleanup = () => {
      doc.removeEventListener("compositionstart", start, true);
      doc.removeEventListener("compositionupdate", update, true);
      doc.removeEventListener("compositionend", end, true);
      doc.defaultView?.removeEventListener("blur", blur);
      doc.defaultView?.removeEventListener("pagehide", pagehide);
      current.reset(null);
    };
    guard = current;
    guards.set(doc, current);
  }
  guard.reporters.add(report);
  report(guard.active !== null);
  return () => {
    const current = guards.get(doc);
    if (!current) return;
    current.reporters.delete(report);
    if (!current.reporters.size) {
      current.cleanup();
      report(false);
      guards.delete(doc);
    }
  };
}

export function isInputCompositionActive(doc: Document = document): boolean {
  return Boolean(guards.get(doc)?.active);
}

const COMMIT_WAIT_MS = 500;

/**
 * Commit the composition in progress the way a focus change does (the browser
 * keeps the preedit as typed text), then let the editors settle, so a save
 * flush that follows sees committed text instead of refusing to run.
 *
 * Resolves `true` when the composition ended on its own after the blur, and
 * `false` when it had to be released after `timeoutMs` without compositionend.
 * Resolves `true` at once when nothing is composing.
 */
export async function commitInputComposition(
  doc: Document = document,
  timeoutMs = COMMIT_WAIT_MS,
): Promise<boolean> {
  const guard = guards.get(doc);
  if (!guard?.active) return true;
  const target = guard.active;
  const ended = new Promise<"ended">(resolve => guard.endWaiters.add(() => resolve("ended")));
  const focused = doc.activeElement;
  if (target instanceof HTMLElement) target.blur();
  if (focused instanceof HTMLElement && focused !== target && focused !== doc.body) focused.blur();
  let timer: ReturnType<typeof setTimeout> | null = null;
  const outcome = await Promise.race([
    ended,
    new Promise<"timeout">(resolve => { timer = setTimeout(() => resolve("timeout"), timeoutMs); }),
  ]);
  if (timer !== null) clearTimeout(timer);
  if (outcome === "timeout") guard.reset(`blur did not end it within ${timeoutMs} ms`);
  // CodeMirror publishes its composition status in a microtask after
  // compositionend, and React commits the last input event's state; a
  // macrotask lets both land before the caller flushes.
  await new Promise<void>(resolve => setTimeout(resolve, 0));
  return outcome === "ended";
}
