// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import { foldedPageLayers, pageLayersField } from "./pageLayersEditing";
import { locateSentence } from "./sentenceProposals";

/**
 * The sentences the AI's newest write changed, marked only while the person
 * asked to see them from Info (UX-WHO-2). A changed sentence also shows what
 * it said before, under its line; an added one is only marked.
 */
export interface AiChangeMark {
  /** The sentence as the AI left it. */
  readonly text: string;
  /** What it said before; null for a sentence the AI added. */
  readonly before: string | null;
}

export interface AiChangeMarkSettings {
  readonly marks: readonly AiChangeMark[];
  /** "Before:" */
  readonly beforeLabel: string;
}

export const setAiChangeMarks = StateEffect.define<AiChangeMarkSettings | null>();

class BeforeWidget extends WidgetType {
  constructor(private readonly before: string, private readonly label: string) {
    super();
  }

  eq(other: BeforeWidget) {
    return other.before === this.before && other.label === this.label;
  }

  toDOM() {
    const root = document.createElement("div");
    root.className = "cm-ai-change-before";
    root.dataset.testid = "ai-change-before";
    const label = document.createElement("span");
    label.className = "cm-ai-change-before-label";
    label.textContent = this.label;
    const text = document.createElement("span");
    text.textContent = this.before;
    root.append(label, text);
    return root;
  }

  ignoreEvent() {
    return true;
  }
}

const changedSentence = Decoration.mark({ class: "cm-ai-changed", attributes: { "data-testid": "ai-changed-sentence" } });

function buildDecorations(state: EditorState, settings: AiChangeMarkSettings | null): DecorationSet {
  if (!settings || settings.marks.length === 0) return Decoration.none;
  const text = state.doc.toString();
  const folds = foldedPageLayers(state);
  const ranges: Range<Decoration>[] = [];
  for (const mark of settings.marks) {
    const at = locateSentence(text, mark.text);
    if (!at) continue;
    if (folds.some((fold) => at.from >= fold.from && at.from < fold.to)) continue;
    ranges.push(changedSentence.range(at.from, at.to));
    if (mark.before) {
      ranges.push(Decoration.widget({
        block: true,
        side: 1,
        widget: new BeforeWidget(mark.before, settings.beforeLabel),
      }).range(state.doc.lineAt(at.to).to));
    }
  }
  return Decoration.set(ranges, true);
}

const aiChangeField = StateField.define<AiChangeMarkSettings | null>({
  create: () => null,
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setAiChangeMarks)) value = effect.value;
    }
    return value;
  },
});

export function aiChangeMarks(initial: AiChangeMarkSettings | null): Extension {
  return [
    aiChangeField.init(() => initial),
    EditorView.decorations.compute(["doc", aiChangeField, pageLayersField], (state) =>
      buildDecorations(state, state.field(aiChangeField))),
    EditorView.theme({
      ".cm-ai-changed": {
        backgroundColor: "color-mix(in srgb, var(--mem-accent-amber) 16%, transparent)",
        borderRadius: "3px",
      },
      ".cm-ai-change-before": {
        display: "flex",
        flexWrap: "wrap",
        gap: "2px 8px",
        margin: "2px 0 10px",
        color: "var(--mem-text-tertiary)",
        fontSize: "var(--mem-text-sm)",
        lineHeight: "1.6",
      },
      ".cm-ai-change-before-label": {
        flex: "none",
      },
    }),
  ];
}
