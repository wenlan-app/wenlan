// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, type DecorationSet } from "@codemirror/view";
import type { WeakReason } from "../../../lib/tauri";
import { foldedPageLayers, pageLayersField } from "./pageLayersEditing";

/**
 * Keep per-sentence authorship and cursor identity available to Info
 * (UX-WHO-1/3/4) without interrupting ordinary prose.
 */
export interface AuthoredSentence {
  readonly hash: string;
  readonly text: string;
  readonly said: "you" | "ai" | null;
  /** Source-check state stays available without inserting a widget into prose. */
  readonly weak: readonly WeakReason[];
}

export interface SentenceAuthorshipSettings {
  readonly sentences: readonly AuthoredSentence[];
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
  ];
}
