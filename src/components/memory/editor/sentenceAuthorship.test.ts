// SPDX-License-Identifier: AGPL-3.0-only
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import { pageLayersEditing } from "./pageLayersEditing";
import {
  leadingWeakReason,
  locateSentences,
  sentenceAt,
  sentenceAuthorship,
  setSentenceAuthorship,
  type SentenceAuthorshipSettings,
} from "./sentenceAuthorship";

const DOC = "We fly on May 12. The flat is a three-minute walk.\n\nTram 28 runs all night. We fly on May 12.";
let view: EditorView | null = null;

function settings(overrides: Partial<SentenceAuthorshipSettings> = {}): SentenceAuthorshipSettings {
  return {
    sentences: [
      { hash: "h1", text: "We fly on May 12.", said: "you", weak: [] },
      { hash: "h2", text: "The flat is a three-minute walk.", said: "ai", weak: [] },
      { hash: "h3", text: "Tram 28 runs all night.", said: "ai", weak: ["stale", "no_citation"] },
      { hash: "h4", text: "We fly on May 12.", said: "ai", weak: [] },
    ],
    labels: {
      you: "Yours",
      ai: "Written by AI",
      weak: (reason) => `Unsupported: ${reason}`,
      recheck: "Ask AI to recheck",
      rechecking: "Rechecking…",
    },
    busyHash: null,
    onRecheck: vi.fn(),
    ...overrides,
  };
}

function mount(initial: SentenceAuthorshipSettings | null) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  view = new EditorView({
    parent,
    state: EditorState.create({ doc: DOC, extensions: [pageLayersEditing(null), sentenceAuthorship(initial)] }),
  });
  return view;
}

const texts = (v: EditorView, selector: string) =>
  Array.from(v.dom.querySelectorAll(selector)).map((el) => el.textContent);

afterEach(() => {
  view?.destroy();
  view = null;
  document.body.innerHTML = "";
});

describe("sentence authorship", () => {
  it("UX-WHO-1: marks yours with a solid line and the AI's with a dotted one, repeated sentences on their own occurrence", () => {
    const v = mount(settings());
    expect(texts(v, ".cm-said-you")).toEqual(["We fly on May 12."]);
    expect(texts(v, ".cm-said-ai")).toEqual(["The flat is a three-minute walk.", "Tram 28 runs all night.", "We fly on May 12."]);
    const ranges = locateSentences(DOC, settings().sentences);
    expect(ranges.map((range) => range.from)).toEqual([0, 18, 52, 76]);
  });

  it("UX-WHO-4: marks a weak sentence right after it and rechecks only that sentence", () => {
    const onRecheck = vi.fn();
    const v = mount(settings({ onRecheck }));
    expect(texts(v, ".cm-sentence-weak-label")).toEqual(["Unsupported: stale"]);
    (v.dom.querySelector(".cm-sentence-weak-recheck") as HTMLButtonElement).click();
    expect(onRecheck).toHaveBeenCalledWith("h3");
  });

  it("UX-WHO-4: leaves a sentence that only lacks a source unmarked in the text", () => {
    const v = mount(settings({ sentences: [{ hash: "h9", text: "Tram 28 runs all night.", said: "ai", weak: ["no_citation"] }] }));
    expect(texts(v, ".cm-sentence-weak")).toEqual([]);
  });

  it("UX-WHO-4: names the most serious reason first", () => {
    expect(leadingWeakReason(["no_citation", "disproven"])).toBe("disproven");
    expect(leadingWeakReason([])).toBeNull();
  });

  it("finds the sentence under the cursor", () => {
    const v = mount(settings());
    expect(sentenceAt(v.state, settings(), 20)).toBe("h2");
    expect(sentenceAt(v.state, settings(), 80)).toBe("h4");
  });

  it("clears every mark when the sentences go away, and never edits the text", () => {
    const v = mount(settings());
    v.dispatch({ effects: setSentenceAuthorship.of(null) });
    expect(texts(v, ".cm-said-you, .cm-said-ai, .cm-sentence-weak")).toEqual([]);
    expect(v.state.doc.toString()).toBe(DOC);
  });
});
