// SPDX-License-Identifier: AGPL-3.0-only
import { StateEffect, StateField, type EditorState, type Extension, type Range } from "@codemirror/state";
import { Decoration, EditorView, WidgetType, type DecorationSet } from "@codemirror/view";
import { foldedPageLayers, pageLayersField } from "./pageLayersEditing";

/**
 * The AI's proposals for the person's sentences, drawn under the sentence they
 * would change (docs/ux/spec.md, UX-WHO-2): one proposal per sentence, with
 * accept or no right there, never a whole-page card. The text itself only
 * changes when the person accepts; the daemon then writes it as their edit.
 */
export interface SentenceProposalView {
  readonly id: string;
  /** The sentence as it read when the AI proposed the change. */
  readonly before: string;
  /** The proposed sentence; empty proposes deleting it. */
  readonly after: string;
}

export interface SentenceProposalLabels {
  readonly suggests: string;
  readonly suggestsRemoving: string;
  readonly accept: string;
  readonly reject: string;
}

export interface SentenceProposalSettings {
  readonly proposals: readonly SentenceProposalView[];
  readonly labels: SentenceProposalLabels;
  /** A proposal whose decision is on its way; its buttons wait. */
  readonly busyId?: string | null;
  readonly onAccept: (id: string) => void;
  readonly onReject: (id: string) => void;
}

export const setSentenceProposals = StateEffect.define<SentenceProposalSettings | null>();

/** Where a proposal's sentence is in the text, or null when it no longer
 *  reads the same (the person changed it; the daemon then refuses the
 *  proposal on accept anyway). */
export function locateSentence(text: string, sentence: string): { from: number; to: number } | null {
  const needle = sentence.trim();
  if (!needle) return null;
  const from = text.indexOf(needle);
  return from < 0 ? null : { from, to: from + needle.length };
}

class ProposalWidget extends WidgetType {
  constructor(
    private readonly proposal: SentenceProposalView,
    private readonly settings: SentenceProposalSettings,
  ) {
    super();
  }

  eq(other: ProposalWidget) {
    return other.proposal.id === this.proposal.id
      && other.proposal.after === this.proposal.after
      && other.settings.labels === this.settings.labels
      && (other.settings.busyId === this.proposal.id) === (this.settings.busyId === this.proposal.id);
  }

  toDOM() {
    const { proposal, settings } = this;
    const busy = settings.busyId === proposal.id;
    const root = document.createElement("div");
    root.className = "cm-sentence-proposal";
    root.dataset.testid = "sentence-proposal";
    root.setAttribute("role", "group");
    root.setAttribute("aria-label", proposal.after ? settings.labels.suggests : settings.labels.suggestsRemoving);

    const label = document.createElement("span");
    label.className = "cm-sentence-proposal-label";
    label.textContent = proposal.after ? settings.labels.suggests : settings.labels.suggestsRemoving;
    root.append(label);
    if (proposal.after) {
      const after = document.createElement("span");
      after.className = "cm-sentence-proposal-text";
      after.textContent = proposal.after;
      root.append(after);
    }

    const actions = document.createElement("span");
    actions.className = "cm-sentence-proposal-actions";
    const button = (text: string, kind: "accept" | "reject", onClick: () => void) => {
      const el = document.createElement("button");
      el.type = "button";
      el.className = `cm-sentence-proposal-button cm-sentence-proposal-button--${kind}`;
      el.dataset.testid = `sentence-proposal-${kind}`;
      el.textContent = text;
      el.disabled = busy;
      el.addEventListener("mousedown", (event) => event.preventDefault());
      el.addEventListener("click", (event) => {
        event.preventDefault();
        if (!busy) onClick();
      });
      return el;
    };
    actions.append(
      button(settings.labels.accept, "accept", () => settings.onAccept(proposal.id)),
      button(settings.labels.reject, "reject", () => settings.onReject(proposal.id)),
    );
    root.append(actions);
    return root;
  }

  ignoreEvent() {
    return true;
  }
}

const proposedSentence = Decoration.mark({ class: "cm-sentence-proposed" });

function buildDecorations(state: EditorState, settings: SentenceProposalSettings | null): DecorationSet {
  if (!settings || settings.proposals.length === 0) return Decoration.none;
  const text = state.doc.toString();
  const folds = foldedPageLayers(state);
  const ranges: Range<Decoration>[] = [];
  const placed = new Set<number>();
  for (const proposal of settings.proposals) {
    const at = locateSentence(text, proposal.before);
    if (!at) continue;
    // A folded sentence is counted on its fold line instead (see PageDetail).
    if (folds.some((fold) => at.from >= fold.from && at.from < fold.to)) continue;
    const lineEnd = state.doc.lineAt(at.to).to;
    // One proposal per sentence; a second one for the same line waits its turn.
    if (placed.has(lineEnd)) continue;
    placed.add(lineEnd);
    ranges.push(proposedSentence.range(at.from, at.to));
    ranges.push(Decoration.widget({ block: true, side: 1, widget: new ProposalWidget(proposal, settings) }).range(lineEnd));
  }
  return Decoration.set(ranges, true);
}

const proposalSettingsField = StateField.define<SentenceProposalSettings | null>({
  create: () => null,
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setSentenceProposals)) value = effect.value;
    }
    return value;
  },
});

export function sentenceProposals(initial: SentenceProposalSettings | null): Extension {
  return [
    proposalSettingsField.init(() => initial),
    EditorView.decorations.compute(["doc", proposalSettingsField, pageLayersField], (state) =>
      buildDecorations(state, state.field(proposalSettingsField))),
    EditorView.theme({
      ".cm-sentence-proposed": {
        backgroundColor: "color-mix(in srgb, var(--mem-accent-indigo) 10%, transparent)",
        borderRadius: "3px",
      },
      ".cm-sentence-proposal": {
        display: "flex",
        flexWrap: "wrap",
        alignItems: "baseline",
        gap: "4px 10px",
        margin: "6px 0 12px",
        padding: "8px 12px",
        borderRadius: "8px",
        background: "color-mix(in srgb, var(--mem-accent-indigo) 7%, transparent)",
        fontSize: "var(--mem-text-sm)",
        lineHeight: "1.6",
      },
      ".cm-sentence-proposal-label": {
        color: "var(--mem-text-tertiary)",
      },
      ".cm-sentence-proposal-text": {
        color: "var(--mem-text)",
        flex: "1 1 16em",
        minWidth: "0",
      },
      ".cm-sentence-proposal-actions": {
        display: "inline-flex",
        gap: "6px",
        marginLeft: "auto",
      },
      ".cm-sentence-proposal-button": {
        border: "none",
        borderRadius: "6px",
        padding: "2px 10px",
        font: "inherit",
        cursor: "pointer",
        whiteSpace: "nowrap",
        background: "transparent",
        color: "var(--mem-text-secondary)",
      },
      ".cm-sentence-proposal-button--accept": {
        background: "color-mix(in srgb, var(--mem-accent-indigo) 18%, transparent)",
        color: "var(--mem-text)",
      },
      ".cm-sentence-proposal-button:hover, .cm-sentence-proposal-button:focus-visible": {
        background: "color-mix(in srgb, var(--mem-accent-indigo) 26%, transparent)",
        color: "var(--mem-text)",
      },
      ".cm-sentence-proposal-button:disabled": {
        cursor: "wait",
        opacity: "0.6",
      },
    }),
  ];
}
