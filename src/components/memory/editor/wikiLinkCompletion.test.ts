// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { history, undo, undoDepth } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { Compartment, EditorSelection, EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { waitFor } from "@testing-library/react";
import { createLinkCompletion, linkCompletionMatches, linkableTitle, refreshLinkCompletion, type LinkCompletionNote } from "./wikiLinkCompletion";
import { pageTitleKey, trimLikeDaemon } from "./pageTitleKey";
import { installCodeMirrorDomPolyfills, pressKey } from "./editorTestUtils";
import { setWritingCompositionActive } from "./writingPresentation";

beforeAll(installCodeMirrorDomPolyfills);
const views: EditorView[] = [];
afterEach(() => { views.splice(0).forEach((view) => { const parent = view.dom.parentElement; view.destroy(); parent?.remove(); }); });
const labels = { label: "Link a page", empty: "No matching pages", useAsSource: "Use as a source", useAsSourceHint: "You'll be told when it changes" };
const notes: LinkCompletionNote[] = [
  { id: "p-roadmap", title: "Roadmap", canUseAsSource: true },
  { id: "p-rust", title: "Rust ownership", canUseAsSource: true },
  { id: "p-trust", title: "Trust model", canUseAsSource: false },
  { id: "p-zh", title: "產品路線", canUseAsSource: true },
];

function make(source = "", position = source.length, initial = notes) {
  let blocked = false;
  let current: readonly LinkCompletionNote[] = initial;
  const onPick = vi.fn();
  const completion = createLinkCompletion({ notes: () => current, labels: () => labels, blocked: (view) => blocked || view.state.readOnly || view.compositionStarted, onPick });
  const onChange = vi.fn();
  const writable = new Compartment();
  const parent = document.body.appendChild(document.createElement("div"));
  const view = new EditorView({ parent, state: EditorState.create({ doc: source, selection: EditorSelection.cursor(position), extensions: [markdown(), history(), completion.extension, writable.of(EditorState.readOnly.of(false)), EditorView.updateListener.of((update) => { if (update.docChanged) onChange(update.state.doc.toString()); })] }) });
  views.push(view);
  view.focus();
  const type = (text: string, userEvent = "input.type") => {
    for (const char of text) {
      const from = view.state.selection.main.head;
      view.dispatch({ changes: { from, insert: char }, selection: EditorSelection.cursor(from + char.length), userEvent });
    }
  };
  const menuElement = () => {
    const id = view.contentDOM.getAttribute("aria-controls");
    return id ? document.getElementById(id) : null;
  };
  return {
    view, type, onChange, onPick, writable, menuElement,
    menu: () => view.state.field(completion.state).menu,
    block: (value: boolean) => { blocked = value; },
    setNotes: (value: readonly LinkCompletionNote[]) => { current = value; view.dispatch({ effects: refreshLinkCompletion.of(null) }); },
  };
}

describe("[[ link completion", () => {
  it("ranks prefix matches, then word starts, then any match, and skips titles a link cannot carry", () => {
    expect(linkCompletionMatches(notes, "ro").map((note) => note.id)).toEqual(["p-roadmap"]);
    expect(linkCompletionMatches(notes, "rust").map((note) => note.id)).toEqual(["p-rust", "p-trust"]);
    expect(linkCompletionMatches(notes, " 路線 ").map((note) => note.id)).toEqual(["p-zh"]);
    expect(linkCompletionMatches(notes, "").map((note) => note.id)).toEqual(["p-roadmap", "p-rust", "p-trust", "p-zh"]);
    const awkward = [
      { id: "a", title: "C# notes", canUseAsSource: true },
      { id: "b", title: "A|B", canUseAsSource: true },
      { id: "c", title: "  ", canUseAsSource: true },
      { id: "d", title: "roadmap", canUseAsSource: true },
      { id: "e", title: "[draft]", canUseAsSource: true },
    ];
    // "roadmap" and "Roadmap" are the same link, which resolves to neither.
    expect(linkCompletionMatches([...notes, ...awkward], "").map((note) => note.id)).toEqual(["p-rust", "p-trust", "p-zh"]);
    expect(linkableTitle("Plain title")).toBe(true);
    expect(linkCompletionMatches(Array.from({ length: 20 }, (_, i) => ({ id: `n${i}`, title: `Note ${i}`, canUseAsSource: true })), "note")).toHaveLength(8);
  });

  it("trims and folds titles exactly as the daemon does", () => {
    // Rust `str::trim` keeps a byte-order mark and strips U+0085; JS `trim()` does the reverse.
    expect(trimLikeDaemon("\ufeffPlan\u0085")).toBe("\ufeffPlan");
    expect(trimLikeDaemon("\u3000路線圖\u00a0")).toBe("路線圖");
    expect(pageTitleKey(" Roadmap\t")).toBe("roadmap");
    // To the daemon "\ufeffPlan" and "Plan" are different titles, so both are offered.
    const bom = [
      { id: "bom", title: "\ufeffPlan", canUseAsSource: true },
      { id: "plain", title: "Plan", canUseAsSource: true },
    ];
    expect(linkCompletionMatches(bom, "").map((note) => note.id)).toEqual(["bom", "plain"]);
    // Inserting keeps the mark, so the label the daemon parses is the stored title.
    const editor = make("See ", undefined, bom);
    editor.type("[[");
    expect(pressKey(editor.view.contentDOM, "Enter").defaultPrevented).toBe(true);
    expect(editor.view.state.doc.toString()).toBe("See [[\ufeffPlan]]");
  });

  it("opens on a typed [[ without changing the text, and filters as the writer types", async () => {
    const editor = make("See ");
    editor.type("[[");
    expect(editor.menu()).toMatchObject({ from: 4, to: 6 });
    expect(editor.view.state.doc.toString()).toBe("See [[");
    await waitFor(() => expect(editor.menuElement()).not.toBeNull());
    const menu = editor.menuElement()!;
    expect(editor.view.dom.contains(menu)).toBe(false);
    expect(menu.getAttribute("aria-label")).toBe("Link a page");
    expect(menu.querySelectorAll('[role="option"]')).toHaveLength(4);
    editor.type("rus");
    expect([...menu.querySelectorAll('[role="option"]')].map((row) => row.getAttribute("aria-label"))).toEqual(["Rust ownership", "Trust model"]);
    expect(editor.view.contentDOM.getAttribute("aria-activedescendant")).toBe(menu.querySelector('[role="option"]')!.id);
    editor.type("zz");
    expect(menu.querySelectorAll('[role="option"]')).toHaveLength(0);
    expect(menu.textContent).toBe("No matching pages");
    expect(editor.view.contentDOM.hasAttribute("aria-activedescendant")).toBe(false);
  });

  it.each([
    ["a single bracket", "", "["],
    ["a third bracket", "[", "[["],
    ["an embed", "!", "[["],
    ["fenced code", "```\n", "[["],
    ["frontmatter", "---\nkey: ", "[["],
  ])("does not open for %s", (_name, source, typed) => {
    const editor = make(source);
    editor.type(typed);
    expect(editor.menu()).toBeNull();
  });

  it("does not open inside inline code", () => {
    const editor = make("`ab`", 3);
    editor.type("[[");
    expect(editor.menu()).toBeNull();
  });

  it("does not open for pasted brackets", () => {
    const editor = make("[");
    editor.type("[", "input.paste");
    expect(editor.menu()).toBeNull();
  });

  it("inserts the chosen title as a plain link with one undo step", () => {
    const editor = make("See ");
    editor.type("[[rust");
    pressKey(editor.view.contentDOM, "ArrowDown");
    editor.onChange.mockClear();
    expect(pressKey(editor.view.contentDOM, "Enter").defaultPrevented).toBe(true);
    expect(editor.view.state.doc.toString()).toBe("See [[Trust model]]");
    expect(editor.view.state.selection.main.head).toBe("See [[Trust model]]".length);
    expect(editor.onChange.mock.calls).toEqual([["See [[Trust model]]"]]);
    expect(editor.onPick).toHaveBeenCalledWith(notes[2], false);
    expect(editor.menu()).toBeNull();
    undo(editor.view);
    expect(editor.view.state.doc.toString()).toBe("See [[rust");
    expect(editor.menu()).toBeNull();
  });

  it("uses a note as a source from its row action or Shift+Enter, and only when offered", async () => {
    const button = make();
    button.type("[[road");
    await waitFor(() => expect(button.menuElement()).not.toBeNull());
    const row = button.menuElement()!.querySelector<HTMLElement>('[role="option"]')!;
    const action = row.querySelector("button")!;
    expect(action.textContent).toBe("Use as a source");
    expect(action.title).toBe("You'll be told when it changes");
    const pointer = new Event("pointerdown", { bubbles: true, cancelable: true });
    action.dispatchEvent(pointer);
    expect(pointer.defaultPrevented).toBe(true);
    action.click();
    expect(button.view.state.doc.toString()).toBe("[[Roadmap]]");
    expect(button.onPick).toHaveBeenCalledTimes(1);
    expect(button.onPick).toHaveBeenCalledWith(notes[0], true);
    expect(document.activeElement).toBe(button.view.contentDOM);

    const keyboard = make();
    keyboard.type("[[rust");
    expect(pressKey(keyboard.view.contentDOM, "Enter", { shiftKey: true }).defaultPrevented).toBe(true);
    expect(keyboard.onPick).toHaveBeenCalledWith(notes[1], true);

    const notOffered = make();
    notOffered.type("[[trust");
    await waitFor(() => expect(notOffered.menuElement()).not.toBeNull());
    expect(notOffered.menuElement()!.querySelector("button")).toBeNull();
    pressKey(notOffered.view.contentDOM, "Enter", { shiftKey: true });
    expect(notOffered.view.state.doc.toString()).toBe("[[Trust model]]");
    expect(notOffered.onPick).toHaveBeenCalledWith(notes[2], false);
  });

  it("selects with a pointer and keeps the editor focused", async () => {
    const editor = make();
    editor.type("[[");
    await waitFor(() => expect(editor.menuElement()).not.toBeNull());
    const rows = editor.menuElement()!.querySelectorAll<HTMLElement>('[role="option"]');
    rows[3].dispatchEvent(new Event("pointerenter"));
    expect(rows[3].getAttribute("aria-selected")).toBe("true");
    expect(editor.menuElement()!.querySelectorAll('[role="option"]')[3]).toBe(rows[3]);
    rows[3].click();
    expect(editor.view.state.doc.toString()).toBe("[[產品路線]]");
    expect(editor.onPick).toHaveBeenCalledWith(notes[3], false);
    expect(document.activeElement).toBe(editor.view.contentDOM);
  });

  it("keeps the pressed row when the notes refresh mid-click", async () => {
    const editor = make();
    editor.type("[[");
    await waitFor(() => expect(editor.menuElement()).not.toBeNull());
    const rows = editor.menuElement()!.querySelectorAll<HTMLElement>('[role="option"]');
    rows[1].dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
    editor.setNotes([{ id: "p-new", title: "A new note", canUseAsSource: true }, ...notes]);
    expect(editor.menuElement()!.querySelectorAll('[role="option"]')[1]).toBe(rows[1]);
    document.dispatchEvent(new Event("pointerup", { bubbles: true }));
    rows[1].click();
    expect(editor.view.state.doc.toString()).toBe("[[Rust ownership]]");
    expect(editor.onPick).toHaveBeenCalledWith(notes[1], false);
  });

  it("shows the new rows once the press ends without a pick", async () => {
    const editor = make();
    editor.type("[[");
    await waitFor(() => expect(editor.menuElement()).not.toBeNull());
    editor.menuElement()!.dispatchEvent(new Event("pointerdown", { bubbles: true, cancelable: true }));
    editor.setNotes([{ id: "p-new", title: "A new note", canUseAsSource: true }, ...notes]);
    expect(editor.menuElement()!.querySelectorAll('[role="option"]')).toHaveLength(4);
    document.dispatchEvent(new Event("pointerup", { bubbles: true }));
    await waitFor(() => expect(editor.menuElement()!.querySelectorAll('[role="option"]')).toHaveLength(5));
  });

  it("opens for 「「 and 【【 from a Chinese input method and writes a real link", () => {
    for (const opener of ["「", "【"]) {
      const editor = make();
      editor.type(opener + opener);
      expect(editor.menu()).not.toBeNull();
      editor.type("產品");
      pressKey(editor.view.contentDOM, "Enter");
      expect(editor.view.state.doc.toString()).toBe("[[產品路線]]");
    }
  });

  it("stays open while an input method composes the title", () => {
    const editor = make();
    editor.type("[[");
    editor.view.dispatch({ effects: setWritingCompositionActive.of(true) });
    editor.type("產", "input.type.compose");
    expect(editor.menu()).not.toBeNull();
    editor.view.dispatch({ effects: setWritingCompositionActive.of(false) });
    pressKey(editor.view.contentDOM, "Enter");
    expect(editor.view.state.doc.toString()).toBe("[[產品路線]]");
  });

  it("follows the host's notes as they arrive", async () => {
    const editor = make("", 0, []);
    editor.type("[[");
    await waitFor(() => expect(editor.menuElement()).not.toBeNull());
    expect(editor.menuElement()!.textContent).toBe("No matching pages");
    editor.setNotes(notes);
    expect(editor.menuElement()!.querySelectorAll('[role="option"]')).toHaveLength(4);
  });

  it("lets Enter make a new line when nothing matches", () => {
    const editor = make();
    editor.type("[[zzz");
    expect(pressKey(editor.view.contentDOM, "Enter").defaultPrevented).toBe(false);
    expect(editor.menu()).toBeNull();
    expect(editor.onPick).not.toHaveBeenCalled();
  });

  it("closes on a closing bracket, a caret move, Escape, Tab, blocking and read-only, without touching history", () => {
    const bracket = make(); bracket.type("[[ro]"); expect(bracket.menu()).toBeNull();
    const caret = make(); caret.type("[[ro"); caret.view.dispatch({ selection: EditorSelection.cursor(1) }); expect(caret.menu()).toBeNull();
    const escape = make(); escape.type("[[");
    const depth = undoDepth(escape.view.state);
    escape.onChange.mockClear();
    expect(pressKey(escape.view.contentDOM, "Escape").defaultPrevented).toBe(true);
    expect(escape.menu()).toBeNull();
    expect(escape.view.state.doc.toString()).toBe("[[");
    expect(undoDepth(escape.view.state)).toBe(depth);
    expect(escape.onChange).not.toHaveBeenCalled();
    escape.type("ro"); expect(escape.menu()).toBeNull();
    const tab = make(); tab.type("[["); expect(pressKey(tab.view.contentDOM, "Tab").defaultPrevented).toBe(false); expect(tab.menu()).toBeNull();
    const blocked = make(); blocked.type("[["); blocked.block(true);
    expect(pressKey(blocked.view.contentDOM, "Enter").defaultPrevented).toBe(false);
    expect(blocked.view.state.doc.toString()).toBe("[[");
    const readonly = make(); readonly.type("[["); readonly.view.dispatch({ effects: readonly.writable.reconfigure(EditorState.readOnly.of(true)) }); expect(readonly.menu()).toBeNull();
  });
});
