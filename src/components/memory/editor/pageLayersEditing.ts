// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import { pageLayerOffsets } from "../page/pageLayers";
import type { PageLength } from "../page/PageReadingLayers";

/**
 * The page layers inside the editor (docs/ux/spec.md, UX-VIEW-2 and
 * UX-VIEW-3). The text stays whole; only the view folds it. "Short" folds the
 * sections of the human part into one line, and the part for AI is one closed
 * line until opened. Opening it is per editor session, like any fold.
 */
export interface PageLayersSettings {
  readonly length: PageLength;
  readonly labels: { readonly aiDetails: string; readonly showFull: string };
  /** Called when the reader opens the folded sections from inside the text. */
  readonly onShowFull?: () => void;
}

interface LayersState {
  readonly settings: PageLayersSettings | null;
  readonly aiOpen: boolean;
  readonly decorations: DecorationSet;
}

export const setPageLayers = StateEffect.define<PageLayersSettings | null>();
const openAiDetails = StateEffect.define<null>();

class FoldLineWidget extends WidgetType {
  constructor(
    private readonly label: string,
    private readonly kind: "short" | "ai",
    private readonly onOpen: (view: EditorView) => void,
  ) {
    super();
  }

  eq(other: FoldLineWidget) {
    return other.label === this.label && other.kind === this.kind;
  }

  toDOM(view: EditorView) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `cm-page-layer-fold cm-page-layer-fold--${this.kind}`;
    button.dataset.testid = this.kind === "ai" ? "page-ai-details-fold" : "page-short-fold";
    button.setAttribute("aria-expanded", "false");
    button.textContent = this.label;
    button.addEventListener("mousedown", (event) => event.preventDefault());
    button.addEventListener("click", (event) => {
      event.preventDefault();
      this.onOpen(view);
    });
    return button;
  }

  ignoreEvent() {
    return true;
  }
}

function buildDecorations(state: EditorState, settings: PageLayersSettings | null, aiOpen: boolean): DecorationSet {
  if (!settings) return Decoration.none;
  const text = state.doc.toString();
  const { sectionStart, aiStart } = pageLayerOffsets(text);
  const ranges = [];
  const end = state.doc.length;
  if (settings.length === "short" && sectionStart !== null) {
    // A block fold ends at the end of a line: the newline before the marker.
    const to = aiStart === null ? end : Math.max(sectionStart, aiStart - 1);
    if (to > sectionStart) {
      ranges.push(Decoration.replace({
        block: true,
        widget: new FoldLineWidget(settings.labels.showFull, "short", () => settings.onShowFull?.()),
      }).range(sectionStart, to));
    }
  }
  if (aiStart !== null && !aiOpen && end > aiStart) {
    ranges.push(Decoration.replace({
      block: true,
      widget: new FoldLineWidget(settings.labels.aiDetails, "ai", (view) => view.dispatch({ effects: openAiDetails.of(null) })),
    }).range(aiStart, end));
  }
  return Decoration.set(ranges);
}

const pageLayersField = StateField.define<LayersState>({
  create: () => ({ settings: null, aiOpen: false, decorations: Decoration.none }),
  update(value, transaction) {
    let { settings, aiOpen } = value;
    let changed = transaction.docChanged;
    for (const effect of transaction.effects) {
      if (effect.is(setPageLayers)) {
        settings = effect.value;
        changed = true;
      } else if (effect.is(openAiDetails)) {
        aiOpen = true;
        changed = true;
      }
    }
    if (!changed) return value;
    return { settings, aiOpen, decorations: buildDecorations(transaction.state, settings, aiOpen) };
  },
  provide: (field) => [
    EditorView.decorations.from(field, (value) => value.decorations),
    EditorView.atomicRanges.of((view) => view.state.field(field).decorations),
  ],
});

export function pageLayersEditing(initial: PageLayersSettings | null): Extension {
  return [
    pageLayersField.init((state) => ({
      settings: initial,
      aiOpen: false,
      decorations: buildDecorations(state, initial, false),
    })),
    EditorView.theme({
      ".cm-page-layer-fold": {
        display: "block",
        margin: "12px 0",
        padding: "0",
        border: "none",
        background: "none",
        color: "var(--mem-text-tertiary)",
        font: "inherit",
        fontSize: "var(--mem-text-sm)",
        cursor: "pointer",
      },
      ".cm-page-layer-fold::before": {
        content: '"▸ "',
      },
      ".cm-page-layer-fold:hover, .cm-page-layer-fold:focus-visible": {
        color: "var(--mem-text)",
      },
    }),
  ];
}
