// SPDX-License-Identifier: AGPL-3.0-only
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { afterEach, describe, expect, it } from "vitest";
import { pageLayersEditing } from "./pageLayersEditing";
import { aiChangeMarks, setAiChangeMarks, type AiChangeMarkSettings } from "./aiChangeMarks";

const DOC = "We fly on May 12. The flat is a three-minute walk.\n\nBook the day trip two weeks ahead.";
let view: EditorView | null = null;

const settings: AiChangeMarkSettings = {
  beforeLabel: "Before:",
  marks: [
    { text: "The flat is a three-minute walk.", before: "The flat is a short walk." },
    { text: "Book the day trip two weeks ahead.", before: null },
    { text: "A sentence someone changed again since.", before: "Old." },
  ],
};

function mount(initial: AiChangeMarkSettings | null) {
  const parent = document.createElement("div");
  document.body.appendChild(parent);
  view = new EditorView({
    parent,
    state: EditorState.create({ doc: DOC, extensions: [pageLayersEditing(null), aiChangeMarks(initial)] }),
  });
  return view;
}

const marked = (v: EditorView) => Array.from(v.dom.querySelectorAll("[data-testid=ai-changed-sentence]")).map((el) => el.textContent);
const befores = (v: EditorView) => Array.from(v.dom.querySelectorAll("[data-testid=ai-change-before]")).map((el) => el.textContent);

afterEach(() => {
  view?.destroy();
  view = null;
  document.body.innerHTML = "";
});

describe("AI change marks", () => {
  it("marks nothing until the person asks", () => {
    const v = mount(null);
    expect(marked(v)).toEqual([]);
    expect(befores(v)).toEqual([]);
  });

  it("marks each changed sentence that still reads the same, with what a changed one said before", () => {
    const v = mount(settings);
    expect(marked(v)).toEqual(["The flat is a three-minute walk.", "Book the day trip two weeks ahead."]);
    expect(befores(v)).toEqual(["Before:The flat is a short walk."]);
  });

  it("clears the marks when the person hides them, and never edits the text", () => {
    const v = mount(settings);
    v.dispatch({ effects: setAiChangeMarks.of(null) });
    expect(marked(v)).toEqual([]);
    expect(v.state.doc.toString()).toBe(DOC);
  });
});
