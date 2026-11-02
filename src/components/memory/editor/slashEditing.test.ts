// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { history, redo, undo, undoDepth } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { Compartment, EditorSelection, EditorState, StateEffect } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { waitFor } from "@testing-library/react";
import { createSlashEditing, refreshSlashLabels, slashItems, type SlashLabels } from "./slashEditing";
import { installCodeMirrorDomPolyfills, pressKey } from "./editorTestUtils";
import { setWritingCompositionActive } from "./writingPresentation";

beforeAll(installCodeMirrorDomPolyfills);
const views: EditorView[] = [];
afterEach(() => { views.splice(0).forEach((view) => { const parent = view.dom.parentElement; view.destroy(); parent?.remove(); }); });
const labels = { label: "Basic editing", ...Object.fromEntries(slashItems.map((item) => [item.id, item.id])) } as SlashLabels;
function make(source = "", position = source.length) {
  let blocked = false;
  let currentLabels = labels;
  const editing = createSlashEditing({ labels: () => currentLabels, blocked: (view) => blocked || view.state.readOnly || view.compositionStarted });
  const onChange = vi.fn();
  const writable = new Compartment();
  const parent = document.body.appendChild(document.createElement("div"));
  const view = new EditorView({ parent, state: EditorState.create({ doc: source, selection: EditorSelection.cursor(position), extensions: [markdown(), history(), editing.extension, writable.of(EditorState.readOnly.of(false)), EditorView.updateListener.of((update) => { if (update.docChanged) onChange(update.state.doc.toString()); })] }) });
  views.push(view);
  view.focus();
  const type = (text: string, userEvent = "input.type") => {
    const from = view.state.selection.main.head;
    view.dispatch({ changes: { from, insert: text }, selection: EditorSelection.cursor(from + text.length), userEvent });
  };
  return { view, type, onChange, writable, menu: () => view.state.field(editing.state).menu, block: (value: boolean) => { blocked = value; }, relabel: (value: SlashLabels) => { currentLabels = value; view.dispatch({ effects: refreshSlashLabels.of(null) }); } };
}

describe("slash editing", () => {
  it("opens only for a typed slash on an empty line and does not alter its source", () => {
    const editor = make("before\n\nafter", 7);
    editor.type("/");
    expect(editor.menu()).toMatchObject({ from: 7, to: 8, active: 0 });
    expect(editor.view.state.doc.toString()).toBe("before\n/\nafter");
    expect(editor.onChange).toHaveBeenCalledTimes(1);
  });

  it.each([
    ["text", "input.type"], ["https://example.test", "input.type"],
    [" ", "input.type"], ["", "input.paste"], ["", "input.type.compose"],
    ["```\n", "input.type"], ["    ", "input.type"],
    ["---\nkey: value\n", "input.type"], ["<script>\n", "input.type"],
  ])("does not open in literal context %j with %s", (source, event) => {
    const editor = make(source);
    editor.type("/", event);
    expect(editor.menu()).toBeNull();
  });

  it("does not open for an initial slash or multiple selections", () => {
    expect(make("/").menu()).toBeNull();
    const editor = make("\n");
    editor.view.dispatch({ effects: StateEffect.appendConfig.of(EditorState.allowMultipleSelections.of(true)) });
    editor.view.dispatch({ selection: EditorSelection.create([EditorSelection.cursor(0), EditorSelection.cursor(1)]) });
    editor.type("/");
    expect(editor.menu()).toBeNull();
  });

  it("excludes blank lines inside fenced and indented code, and allows a line after closed frontmatter", () => {
    const fence = make("```\n\n```", 4); fence.type("/"); expect(fence.menu()).toBeNull();
    const code = make("    first\n\n    second", 10); code.type("/"); expect(code.menu()).toBeNull();
    const frontmatter = make("---\nkey: value\n---\n"); frontmatter.type("/"); expect(frontmatter.menu()).not.toBeNull();
  });

  it("keeps a themed body tooltip stable while changing labels and selecting with a pointer", async () => {
    const editor = make();
    editor.type("/");
    const id = editor.view.contentDOM.getAttribute("aria-controls")!;
    await waitFor(() => expect(document.getElementById(id)).not.toBeNull());
    const menu = document.getElementById(id)!;
    expect(editor.view.dom.contains(menu)).toBe(false);
    for (const themeClass of editor.view.themeClasses.split(" ")) expect(menu.parentElement?.classList.contains(themeClass)).toBe(true);
    expect(menu.querySelectorAll('[role="option"]')).toHaveLength(slashItems.length);
    editor.onChange.mockClear();
    editor.relabel({ ...labels, label: "基本編輯", heading2: "二級標題" });
    expect(document.getElementById(id)).toBe(menu);
    expect(menu.getAttribute("aria-label")).toBe("基本編輯");
    const row = menu.querySelectorAll<HTMLElement>('[role="option"]')[1];
    expect(row.textContent).toBe("二級標題");
    row.dispatchEvent(new Event("pointerenter"));
    expect(document.getElementById(id)).toBe(menu);
    expect(editor.onChange).not.toHaveBeenCalled();
    const pointer = new Event("pointerdown", { bubbles: true, cancelable: true });
    row.dispatchEvent(pointer);
    expect(pointer.defaultPrevented).toBe(true);
    row.click();
    expect(editor.view.state.doc.toString()).toBe("## ");
    expect(document.activeElement).toBe(editor.view.contentDOM);
    expect(document.getElementById(id)).toBeNull();
    expect(editor.view.contentDOM.hasAttribute("aria-activedescendant")).toBe(false);
    expect(editor.onChange.mock.calls).toEqual([["## "]]);
  });

  it.each(slashItems.filter((item) => item.id !== "pageLink"))("accepts $id as one source change with isolated undo and redo", (item) => {
    const editor = make("before\n\nafter", 7);
    editor.type("/");
    for (let i = 0; i < slashItems.indexOf(item); i++) pressKey(editor.view.contentDOM, "ArrowDown");
    editor.onChange.mockClear();
    expect(pressKey(editor.view.contentDOM, "Enter").defaultPrevented).toBe(true);
    expect(editor.view.state.doc.toString()).toBe(`before\n${item.insert}\nafter`);
    expect(editor.view.state.selection.main.head).toBe(7 + item.caret);
    expect(editor.onChange.mock.calls).toEqual([[`before\n${item.insert}\nafter`]]);
    expect(editor.menu()).toBeNull();
    undo(editor.view);
    expect(editor.view.state.doc.toString()).toBe("before\n/\nafter");
    expect(editor.menu()).toBeNull();
    redo(editor.view);
    expect(editor.view.state.doc.toString()).toBe(`before\n${item.insert}\nafter`);
    // CodeMirror history maps its saved pre-command caret on redo. For a
    // multiline skeleton it lands at the end; the source remains exact.
    expect(editor.view.state.selection.main.head).toBe(7 + item.insert.length);
    editor.type("x");
    undo(editor.view);
    expect(editor.view.state.doc.toString()).toBe(`before\n${item.insert}\nafter`);
  });

  it("Link to page types [[ so the page picker opens, and one undo brings back the slash", () => {
    const editor = make("before\n\nafter", 7);
    editor.type("/");
    editor.type("link");
    expect(editor.menu()?.query).toBe("link");
    expect(pressKey(editor.view.contentDOM, "Enter").defaultPrevented).toBe(true);
    expect(editor.view.state.doc.toString()).toBe("before\n[[\nafter");
    expect(editor.view.state.selection.main.head).toBe(9);
    undo(editor.view);
    expect(editor.view.state.doc.toString()).toBe("before\n/link\nafter");
  });

  it("narrows the list as you type after the slash and closes when nothing matches", async () => {
    const editor = make();
    editor.type("/");
    editor.type("head");
    const id = editor.view.contentDOM.getAttribute("aria-controls")!;
    await waitFor(() => expect(document.getElementById(id)).not.toBeNull());
    const visible = [...document.getElementById(id)!.querySelectorAll<HTMLElement>('[role="option"]')].filter((row) => !row.hidden).map((row) => row.textContent);
    expect(visible).toEqual(["heading1", "heading2", "heading3"]);
    pressKey(editor.view.contentDOM, "ArrowUp");
    expect(pressKey(editor.view.contentDOM, "Enter").defaultPrevented).toBe(true);
    expect(editor.view.state.doc.toString()).toBe("### ");
    const none = make();
    none.type("/");
    none.type("zzz");
    expect(none.menu()).toBeNull();
    expect(none.view.state.doc.toString()).toBe("/zzz");
    const spaced = make();
    spaced.type("/");
    spaced.type(" ");
    expect(spaced.menu()).toBeNull();
  });

  it("navigates and dismisses without changing document or history and stays dismissed", () => {
    const editor = make();
    editor.type("/");
    const depth = undoDepth(editor.view.state);
    editor.onChange.mockClear();
    pressKey(editor.view.contentDOM, "ArrowUp");
    expect(editor.menu()?.active).toBe(slashItems.length - 1);
    expect(pressKey(editor.view.contentDOM, "Escape").defaultPrevented).toBe(true);
    editor.view.dispatch({});
    expect(editor.menu()).toBeNull();
    expect(editor.view.state.doc.toString()).toBe("/");
    expect(undoDepth(editor.view.state)).toBe(depth);
    expect(editor.onChange).not.toHaveBeenCalled();
  });

  it("closes on text, caret, Tab, composition and readonly transitions", () => {
    const text = make(); text.type("/"); text.type("中"); expect(text.menu()).toBeNull();
    const caret = make(); caret.type("/"); caret.view.dispatch({ selection: EditorSelection.cursor(0) }); expect(caret.menu()).toBeNull();
    const tab = make(); tab.type("/"); expect(pressKey(tab.view.contentDOM, "Tab").defaultPrevented).toBe(true); expect(tab.menu()).toBeNull();
    const ime = make(); ime.type("/"); ime.view.dispatch({ effects: setWritingCompositionActive.of(true) }); expect(ime.menu()).toBeNull(); ime.type("/", "input.type.compose"); ime.view.dispatch({ effects: setWritingCompositionActive.of(false) }); expect(ime.menu()).toBeNull();
    const readonly = make(); readonly.type("/"); readonly.view.dispatch({ effects: readonly.writable.reconfigure(EditorState.readOnly.of(true)) }); expect(readonly.menu()).toBeNull();
  });
});
