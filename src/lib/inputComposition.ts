// SPDX-License-Identifier: AGPL-3.0-only
type Reporter = (active: boolean) => void;
type Guard = { active: EventTarget | null; reporters: Set<Reporter>; cleanup: () => void };
const guards = new WeakMap<Document, Guard>();

/** Observe the browser's IME lifecycle for application navigation, including native inputs. */
export function observeInputComposition(report: Reporter, doc: Document = document): () => void {
  let guard = guards.get(doc);
  if (!guard) {
    const current: Guard = { active: null, reporters: new Set(), cleanup: () => {} };
    const publish = (target: EventTarget | null) => {
      const previous = current.active !== null;
      current.active = target;
      if (previous !== (target !== null)) {
        for (const listener of current.reporters) listener(target !== null);
      }
    };
    // A replaced editor may be detached without receiving compositionend.
    // Observe removals only while composing, so a stale native bit cannot block shortcuts.
    const removals = new MutationObserver(() => {
      if (current.active instanceof Node && !current.active.isConnected) {
        removals.disconnect();
        publish(null);
      }
    });
    const start = (event: CompositionEvent) => {
      publish(event.target);
      removals.observe(doc, { childList: true, subtree: true });
    };
    const end = (event: CompositionEvent) => {
      if (current.active !== event.target) return;
      removals.disconnect();
      publish(null);
    };
    const reset = () => { removals.disconnect(); publish(null); };
    doc.addEventListener("compositionstart", start, true);
    doc.addEventListener("compositionend", end, true);
    doc.defaultView?.addEventListener("pagehide", reset);
    current.cleanup = () => {
      doc.removeEventListener("compositionstart", start, true);
      doc.removeEventListener("compositionend", end, true);
      doc.defaultView?.removeEventListener("pagehide", reset);
      reset();
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
