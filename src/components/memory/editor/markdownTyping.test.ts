// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { history, undo } from "@codemirror/commands";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { AUTO_CLOSED_BRACKETS, indentMarkdown, markdownBrackets, outdentMarkdown } from "./markdownTyping";
import { installCodeMirrorDomPolyfills, pressKey, typeText } from "./editorTestUtils";

beforeAll(installCodeMirrorDomPolyfills);
const views: EditorView[] = [];
afterEach(() => { views.splice(0).forEach((view) => { view.dom.parentElement?.remove(); view.destroy(); }); });

/** `|` marks the caret; `|text|` marks a selection from the first bar to the second. */
function make(marked: string, readOnly = false): EditorView {
  const first = marked.indexOf("|");
  const second = marked.indexOf("|", first + 1);
  const doc = marked.replace(/\|/g, "");
  const selection = second < 0 ? EditorSelection.cursor(first) : EditorSelection.range(first, second - 1);
  const view = new EditorView({
    parent: document.body.appendChild(document.createElement("div")),
    state: EditorState.create({ doc, selection, extensions: [
      history(), markdown({ base: markdownLanguage }), markdownBrackets(), EditorState.readOnly.of(readOnly),
    ] }),
  });
  views.push(view);
  return view;
}

function shown(view: EditorView): string {
  const text = view.state.doc.toString();
  const { from, to } = view.state.selection.main;
  return from === to ? `${text.slice(0, from)}|${text.slice(from)}` : `${text.slice(0, from)}|${text.slice(from, to)}|${text.slice(to)}`;
}

describe("Tab and Shift+Tab", () => {
  it.each([
    ["nests a bullet under the one above", "- a\n- b|", "- a\n  - b|"],
    ["takes the item's children along", "- a\n- b|\n  - c\n    - d\n- e", "- a\n  - b|\n    - c\n      - d\n- e"],
    ["lines a numbered item up with its sibling's text and keeps its number", "1. a\n2. b|", "1. a\n   2. b|"],
    ["nests a task", "- [ ] a\n- [x] b|", "- [ ] a\n  - [x] b|"],
    ["nests an empty item made by Enter", "- a\n- |", "- a\n  - |"],
    ["keeps tabs in a list indented with tabs", "- a\n\t- b\n\t- c|", "- a\n\t- b\n\t\t- c|"],
    ["nests every selected item by the same step", "- a\n- |b\n- c|", "- a\n  - |b\n  - c|"],
    ["inserts the indent unit on a plain line", "pl|ain", "pl  |ain"],
    ["indents selected plain lines", "|one\ntwo|", "  |one\n  two|"],
    ["treats a list inside a code fence as plain text", "```\n- a\n- b|\n```", "```\n- a\n- b  |\n```"],
  ])("Tab %s", (_name, before, after) => {
    const view = make(before);
    expect(indentMarkdown(view)).toBe(true);
    expect(shown(view)).toBe(after);
  });

  it.each([
    ["the first item of a list", "- a|\n- b"],
    ["an item already one level below the item above", "- a\n  - b|"],
  ])("Tab on %s changes nothing but still keeps focus in the editor", (_name, source) => {
    const view = make(source);
    expect(indentMarkdown(view)).toBe(true);
    expect(shown(view)).toBe(source);
  });

  it.each([
    ["moves a nested item out to its parent's column", "- a\n  - b\n    - c|\n  - d", "- a\n  - b\n  - c|\n  - d"],
    ["takes the item's children along", "- a\n  - b|\n    - c\n- d", "- a\n- b|\n  - c\n- d"],
    ["moves a numbered item out", "1. a\n   1. b|", "1. a\n1. b|"],
    ["removes one tab in a list indented with tabs", "- a\n\t- b|", "- a\n- b|"],
    ["removes an indent from a plain line", "  pl|ain", "pl|ain"],
  ])("Shift+Tab %s", (_name, before, after) => {
    const view = make(before);
    expect(outdentMarkdown(view)).toBe(true);
    expect(shown(view)).toBe(after);
  });

  it("is consumed at the margin and undone in one step", () => {
    const margin = make("- a|");
    expect(outdentMarkdown(margin)).toBe(true);
    expect(shown(margin)).toBe("- a|");
    const view = make("- a\n- b|\n  - c");
    indentMarkdown(view);
    expect(view.state.doc.toString()).toBe("- a\n  - b\n    - c");
    undo(view);
    expect(view.state.doc.toString()).toBe("- a\n- b\n  - c");
  });

  it("leaves a read-only editor alone so Tab can move focus", () => {
    const view = make("- a\n- b|", true);
    expect(indentMarkdown(view)).toBe(false);
    expect(outdentMarkdown(view)).toBe(false);
  });
});

describe("closing brackets and quotes", () => {
  it("pairs ( [ { \" and backticks, not apostrophes or full-width brackets", () => {
    expect(AUTO_CLOSED_BRACKETS).toEqual(["(", "[", "{", '"', "`"]);
    for (const [typed, result] of [
      ["(", "(|)"], ["[", "[|]"], ["{", "{|}"], ['"', '"|"'], ["`", "`|`"],
      ["don'", "don'|"], ["「", "「|"], ["（", "（|"], ["［", "［|"],
    ]) {
      const view = make("|");
      typeText(view, typed);
      expect(shown(view), typed).toBe(result);
    }
  });

  it("does not pair in front of a word", () => {
    const view = make("|word");
    typeText(view, "(");
    expect(shown(view)).toBe("(|word");
  });

  it("types over the closer it inserted", () => {
    for (const [typed, result] of [["(a)", "(a)|"], ["[b]", "[b]|"], ['say "hi"', 'say "hi"|'], ["`code`", "`code`|"]]) {
      const view = make("|");
      typeText(view, typed);
      expect(shown(view), typed).toBe(result);
    }
  });

  it("removes an empty pair with one Backspace", () => {
    const view = make("|");
    typeText(view, "(");
    expect(pressKey(view.contentDOM, "Backspace").defaultPrevented).toBe(true);
    expect(view.state.doc.toString()).toBe("");
  });

  it("makes `[[` one pair of double brackets", () => {
    const view = make("see |");
    typeText(view, "[[");
    expect(shown(view)).toBe("see [[|]]");
    typeText(view, "Page]]");
    expect(shown(view)).toBe("see [[Page]]|");
  });

  it("opens a code fence with exactly three backticks", () => {
    for (const before of ["|", "- a\n|", "> |"]) {
      const view = make(before);
      typeText(view, "```js");
      expect(shown(view), before).toBe(`${before.replace("|", "")}\`\`\`js|`);
    }
  });

  it.each([
    ["*", "*|hello|* world"],
    ["**", "**|hello|** world"],
    ["_", "_|hello|_ world"],
    ["[", "[|hello|] world"],
    ["[[", "[[|hello|]] world"],
    ["(", "(|hello|) world"],
    ['"', '"|hello|" world'],
    ["`", "`|hello|` world"],
  ])("wraps a selection in %s", (typed, result) => {
    const view = make("|hello| world");
    typeText(view, typed);
    expect(shown(view)).toBe(result);
  });

  it("replaces a selection that spans lines when * is typed", () => {
    const view = make("|one\ntwo|");
    typeText(view, "*");
    expect(view.state.doc.toString()).toBe("*");
  });

  it("neither pairs nor wraps while an input method is composing", () => {
    const view = make("|hello| ");
    view.contentDOM.dispatchEvent(new CompositionEvent("compositionstart", { bubbles: true }));
    expect(view.compositionStarted).toBe(true);
    typeText(view, "*");
    expect(view.state.doc.toString()).toBe("* ");
    typeText(view, "(");
    expect(view.state.doc.toString()).toBe("*( ");
  });
});
