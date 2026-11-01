// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import type { WeakReason } from "../../../lib/tauri";
import { foldedPageLayers, pageLayersField } from "./pageLayersEditing";

/**
 * Who said each sentence (docs/ux/spec.md, UX-WHO-1): yours, including the
 * ones you accepted, carry a thin solid line; the AI's a thin dotted one.
 * Visible, but quiet. A sentence whose source no longer backs it gets a
 * small mark right after it, with one press to recheck only that sentence
 * (UX-WHO-4).
 */
export interface AuthoredSentence {
  readonly hash: string;
  readonly text: string;
  readonly said: "you" | "ai" | null;
  /** Why the sentence is weakly backed; empty when it is fine. */
  readonly weak: readonly WeakReason[];
}

export interface SentenceAuthorshipSettings {
  readonly sentences: readonly AuthoredSentence[];
  readonly labels: {
    readonly you: string;
    readonly ai: string;
    readonly weak: (reason: WeakReason) => string;
    readonly recheck: string;
    readonly rechecking: string;
  };
  /** The sentence being rechecked right now, if any. */
  readonly busyHash: string | null;
  readonly onRecheck: (hash: string) => void;
  /** The sentence the cursor is in, for Info's "this sentence" (UX-WHO-3). */
  readonly onCursorSentence?: (hash: string | null) => void;
}

export const setSentenceAuthorship = StateEffect.define<SentenceAuthorshipSettings | null>();

/** The weak reason worth naming first. */
export function leadingWeakReason(weak: readonly WeakReason[]): WeakReason | null {
  for (const reason of ["disproven", "unsupported", "stale", "no_citation"] as const) {
    if (weak.includes(reason)) return reason;
  }
  return null;
}

class WeakWidget extends WidgetType {
  constructor(
    private readonly hash: string,
    private readonly reason: WeakReason,
    private readonly settings: SentenceAuthorshipSettings,
  ) {
    super();
  }

  eq(other: WeakWidget) {
    return other.hash === this.hash
      && other.reason === this.reason
      && other.settings.labels === this.settings.labels
      && (other.settings.busyHash === this.hash) === (this.settings.busyHash === this.hash);
  }

  toDOM() {
    const { hash, reason, settings } = this;
    const busy = settings.busyHash === hash;
    const root = document.createElement("span");
    root.className = "cm-sentence-weak";
    root.dataset.testid = "sentence-weak";
    const label = document.createElement("span");
    label.className = "cm-sentence-weak-label";
    label.textContent = settings.labels.weak(reason);
    const button = document.createElement("button");
    button.type = "button";
    button.className = "cm-sentence-weak-recheck";
    button.disabled = busy;
    button.textContent = busy ? settings.labels.rechecking : settings.labels.recheck;
    button.addEventListener("mousedown", (event) => event.preventDefault());
    button.addEventListener("click", (event) => {
      event.preventDefault();
      settings.onRecheck(hash);
    });
    root.append(label, button);
    return root;
  }

  ignoreEvent() {
    return true;
  }
}

const saidYou = Decoration.mark({ class: "cm-said-you" });
const saidAi = Decoration.mark({ class: "cm-said-ai" });

/** Each sentence's place in the document, searched in page order so a
 *  repeated sentence lands on its own occurrence. */
export function locateSentences(text: string, sentences: readonly AuthoredSentence[]) {
  const located: { sentence: AuthoredSentence; from: number; to: number }[] = [];
  let cursor = 0;
  for (const sentence of sentences) {
    const needle = sentence.text.trim();
    if (!needle) continue;
    let from = text.indexOf(needle, cursor);
    if (from < 0) from = text.indexOf(needle);
    if (from < 0) continue;
    located.push({ sentence, from, to: from + needle.length });
    cursor = from + needle.length;
  }
  return located;
}

function buildDecorations(state: EditorState, settings: SentenceAuthorshipSettings | null): DecorationSet {
  if (!settings || settings.sentences.length === 0) return Decoration.none;
  const folds = foldedPageLayers(state);
  const ranges: Range<Decoration>[] = [];
  for (const { sentence, from, to } of locateSentences(state.doc.toString(), settings.sentences)) {
    if (folds.some((fold) => from >= fold.from && from < fold.to)) continue;
    if (sentence.said === "you") ranges.push(saidYou.range(from, to));
    if (sentence.said === "ai") ranges.push(saidAi.range(from, to));
    // "No source" alone stays in Info: most AI sentences carry no citation,
    // and a mark after each would drown the ones whose facts changed.
    const reason = leadingWeakReason(sentence.weak);
    if (reason && reason !== "no_citation") {
      ranges.push(Decoration.widget({ side: 1, widget: new WeakWidget(sentence.hash, reason, settings) }).range(to));
    }
  }
  return Decoration.set(ranges, true);
}

const settingsField = StateField.define<SentenceAuthorshipSettings | null>({
  create: () => null,
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setSentenceAuthorship)) value = effect.value;
    }
    return value;
  },
});

/** Rebuilt when the sentences or the folds change; mapped through typing in
 *  between, so a keystroke never rescans the page. */
const decorationsField = StateField.define<DecorationSet>({
  create: (state) => buildDecorations(state, state.field(settingsField)),
  update(decorations, transaction) {
    const settingsChanged = transaction.startState.field(settingsField) !== transaction.state.field(settingsField);
    const foldsChanged = transaction.startState.field(pageLayersField) !== transaction.state.field(pageLayersField);
    if (settingsChanged || foldsChanged) return buildDecorations(transaction.state, transaction.state.field(settingsField));
    return transaction.docChanged ? decorations.map(transaction.changes) : decorations;
  },
  provide: (field) => EditorView.decorations.from(field),
});

/** The sentence that holds `position`, if any. */
export function sentenceAt(state: EditorState, settings: SentenceAuthorshipSettings | null, position: number): string | null {
  if (!settings) return null;
  for (const { sentence, from, to } of locateSentences(state.doc.toString(), settings.sentences)) {
    if (position >= from && position <= to) return sentence.hash;
  }
  return null;
}

const reportCursorSentence = EditorView.updateListener.of((update) => {
  if (!update.selectionSet && !update.focusChanged) return;
  const settings = update.state.field(settingsField);
  if (!settings?.onCursorSentence || !update.view.hasFocus) return;
  const hash = sentenceAt(update.state, settings, update.state.selection.main.head);
  const previous = cursorHashes.get(update.view);
  if (previous === hash) return;
  cursorHashes.set(update.view, hash);
  settings.onCursorSentence(hash);
});

const cursorHashes = new WeakMap<EditorView, string | null>();

export function sentenceAuthorship(initial: SentenceAuthorshipSettings | null): Extension {
  return [
    settingsField.init(() => initial),
    decorationsField,
    reportCursorSentence,
    EditorView.theme({
      ".cm-said-you": {
        textDecorationLine: "underline",
        textDecorationStyle: "solid",
        textDecorationThickness: "1px",
        textUnderlineOffset: "5px",
        textDecorationColor: "color-mix(in srgb, var(--mem-accent-sage) 55%, transparent)",
      },
      ".cm-said-ai": {
        textDecorationLine: "underline",
        textDecorationStyle: "dotted",
        textDecorationThickness: "1px",
        textUnderlineOffset: "5px",
        textDecorationColor: "color-mix(in srgb, var(--mem-accent-amber) 55%, transparent)",
      },
      ".cm-sentence-weak": {
        display: "inline-flex",
        alignItems: "center",
        gap: "6px",
        marginLeft: "8px",
        verticalAlign: "1px",
        whiteSpace: "nowrap",
      },
      ".cm-sentence-weak-label": {
        padding: "0 8px",
        borderRadius: "99px",
        fontSize: "var(--mem-text-xs, 12px)",
        lineHeight: "1.6",
        color: "var(--mem-accent-amber)",
        background: "color-mix(in srgb, var(--mem-accent-amber) 12%, transparent)",
      },
      ".cm-sentence-weak-recheck": {
        padding: "0 6px",
        border: "0",
        borderRadius: "6px",
        background: "transparent",
        color: "var(--mem-text-secondary)",
        font: "inherit",
        fontSize: "var(--mem-text-xs, 12px)",
        lineHeight: "1.6",
        cursor: "pointer",
        textDecoration: "underline",
        textUnderlineOffset: "3px",
      },
      ".cm-sentence-weak-recheck:hover": { color: "var(--mem-text)" },
      ".cm-sentence-weak-recheck:disabled": { cursor: "default", opacity: "0.6" },
    }),
  ];
}
