// SPDX-License-Identifier: AGPL-3.0-only
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { pageLayersEditing, type PageLayersSettings } from "./pageLayersEditing";
import { sentenceProposals, setSentenceProposals, type SentenceProposalSettings } from "./sentenceProposals";
import { proposalsInFolds } from "../page/proposalsInFolds";

const DOC = "The tagline is short. It names a noun.\n\n## Why\n\nPeople skim.\n\n## Details for AI\n\nRaw notes.";
const labels = { suggests: "AI suggests:", suggestsRemoving: "AI suggests removing this sentence.", accept: "Accept", reject: "No" };
const layerLabels = { aiDetails: "Details for AI", showFull: "Show the full page" };
let view: EditorView | null = null;

function settings(overrides: Partial<SentenceProposalSettings> = {}): SentenceProposalSettings {
  return {
    proposals: [{ id: "sp_1", before: "It names a noun.", after: "It names a concrete noun." }],
    labels,
    onAccept: vi.fn(),
    onReject: vi.fn(),
    ...overrides,
  };
}

function mount(proposals: SentenceProposalSettings | null, layers: PageLayersSettings | null = null) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  view = new EditorView({
    parent,
    state: EditorState.create({ doc: DOC, extensions: [pageLayersEditing(layers), sentenceProposals(proposals)] }),
  });
  return view;
}

const widgets = (v: EditorView) => Array.from(v.dom.querySelectorAll<HTMLElement>("[data-testid=sentence-proposal]"));

afterEach(() => {
  view?.destroy();
  view = null;
  document.body.innerHTML = "";
});

describe("sentenceProposals", () => {
  it("UX-WHO-2 puts the proposal under its sentence with accept or no, leaving the text as it is", () => {
    const current = settings();
    const v = mount(current);
    const [widget] = widgets(v);
    expect(widget.textContent).toContain("AI suggests:");
    expect(widget.textContent).toContain("It names a concrete noun.");
    // Drawn after the sentence's line, before the next section.
    const line = v.dom.querySelector(".cm-sentence-proposed");
    expect(line?.textContent).toBe("It names a noun.");
    expect(widget.compareDocumentPosition(line!) & Node.DOCUMENT_POSITION_PRECEDING).toBeTruthy();

    widget.querySelector<HTMLButtonElement>("[data-testid=sentence-proposal-accept]")!.click();
    expect(current.onAccept).toHaveBeenCalledWith("sp_1");
    widget.querySelector<HTMLButtonElement>("[data-testid=sentence-proposal-reject]")!.click();
    expect(current.onReject).toHaveBeenCalledWith("sp_1");
    expect(v.state.doc.toString()).toBe(DOC);
  });

  it("says when the AI proposes removing a sentence", () => {
    const v = mount(settings({ proposals: [{ id: "sp_2", before: "People skim.", after: "" }] }));
    expect(widgets(v)[0].textContent).toContain("AI suggests removing this sentence.");
  });

  it("waits while a decision is on its way", () => {
    const v = mount(settings({ busyId: "sp_1" }));
    const buttons = widgets(v)[0].querySelectorAll("button");
    expect(Array.from(buttons).every((button) => button.disabled)).toBe(true);
  });

  it("draws nothing for a sentence that no longer reads the same", () => {
    expect(widgets(mount(settings({ proposals: [{ id: "sp_3", before: "Gone.", after: "Back." }] })))).toHaveLength(0);
  });

  it("follows new proposals and clears them", () => {
    const v = mount(null);
    expect(widgets(v)).toHaveLength(0);
    v.dispatch({ effects: setSentenceProposals.of(settings()) });
    expect(widgets(v)).toHaveLength(1);
    v.dispatch({ effects: setSentenceProposals.of(null) });
    expect(widgets(v)).toHaveLength(0);
  });

  it("does not draw inside a fold; the fold line carries the count instead", () => {
    const folded = settings({ proposals: [{ id: "sp_4", before: "People skim.", after: "People skim first." }] });
    expect(widgets(mount(folded, { length: "short", labels: layerLabels }))).toHaveLength(0);
    expect(proposalsInFolds(DOC, folded.proposals, "short")).toEqual({ short: 1, ai: 0 });
    expect(proposalsInFolds(DOC, folded.proposals, "full")).toEqual({ short: 0, ai: 0 });
    view?.destroy();
    expect(widgets(mount(folded, { length: "full", labels: layerLabels }))).toHaveLength(1);
  });

  it("counts a proposal in the part for AI on that part's line", () => {
    expect(proposalsInFolds(DOC, [{ id: "sp_5", before: "Raw notes.", after: "Notes." }], "full")).toEqual({ short: 0, ai: 1 });
  });
});
