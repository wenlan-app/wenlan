// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { ensureSyntaxTree, forceParsing, syntaxTree, syntaxTreeAvailable } from "@codemirror/language";
import { markdown } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { setWritingCompositionActive } from "./writingPresentation";
import { createWikiLinkEditing, parseResolvedWikiLinks, setResolvedWikiLinkTargets, setEditorReferenceContext, type EditorReferenceContext } from "./wikiLinkEditing";
import { installCodeMirrorDomPolyfills } from "./editorTestUtils";

beforeAll(installCodeMirrorDomPolyfills);
const views: EditorView[] = [];
afterEach(() => { views.splice(0).forEach((view) => { const parent = view.dom.parentElement; view.destroy(); parent?.remove(); }); });

const targets = new Map([["target", "page-target"], ["city walk", "page-city"]]);

function make(source: string, initialTargets: ReadonlyMap<string, string> = targets, context: EditorReferenceContext = {}) {
  const onReferenceActivate = vi.fn();
  const onReferencePreview = vi.fn();
  const onActivate = vi.fn();
  const onPreview = vi.fn();
  const onPreviewLeave = vi.fn();
  const parent = document.body.appendChild(document.createElement("div"));
  const view = new EditorView({
    parent,
    state: EditorState.create({ doc: source, extensions: [markdown(), createWikiLinkEditing(initialTargets, { onActivate, onPreview, onPreviewLeave, onReferenceActivate, onReferencePreview }, context)] }),
  });
  views.push(view);
  expect(forceParsing(view, source.length, 1000)).toBe(true);
  return { view, onActivate, onPreview, onPreviewLeave, onReferenceActivate, onReferencePreview };
}

describe("wiki link editing", () => {
  it("waits for a complete Markdown tree before exposing wiki links", () => {
    const codeLinks = Array.from({ length: 400 }, () => "[[Target]]").join("\n");
    const source = ["[[Target]]", "", "```md", codeLinks, "```"].join("\n");
    let now = 0;
    const dateNow = vi.spyOn(Date, "now").mockImplementation(() => {
      now += 100;
      return now;
    });

    let view: EditorView;
    try {
      const parent = document.body.appendChild(document.createElement("div"));
      view = new EditorView({
        parent,
        state: EditorState.create({ doc: source, extensions: [markdown(), createWikiLinkEditing(targets, {
          onActivate: vi.fn(), onPreview: vi.fn(), onPreviewLeave: vi.fn(),
        })] }),
      });
      views.push(view);
    } finally {
      dateNow.mockRestore();
    }

    expect(syntaxTreeAvailable(view!.state, source.length)).toBe(false);
    expect(view!.contentDOM.querySelectorAll("a[data-wiki-page-id]")).toHaveLength(0);

    forceParsing(view!, source.length, 1000);
    expect(syntaxTreeAvailable(view!.state, source.length)).toBe(true);
    expect([...view!.contentDOM.querySelectorAll<HTMLAnchorElement>("a[data-wiki-page-id]")].map((anchor) => anchor.textContent))
      .toEqual(["Target"]);
    expect(view!.state.doc.toString()).toBe(source);
  });

  it("uses the completed parser-context tree when the state snapshot is stale", () => {
    const codeLinks = Array.from({ length: 80 }, () => "[[Target]]").join("\n");
    const source = ["[[Target]]", "", "```md", codeLinks, "```"].join("\n");
    const extension = createWikiLinkEditing(targets, {
      onActivate: vi.fn(), onPreview: vi.fn(), onPreviewLeave: vi.fn(),
    });
    let now = 0;
    const dateNow = vi.spyOn(Date, "now").mockImplementation(() => {
      now += 100;
      return now;
    });

    let state: EditorState;
    try {
      state = EditorState.create({ doc: source, extensions: [markdown(), extension] });
    } finally {
      dateNow.mockRestore();
    }

    const stateTree = syntaxTree(state!);
    expect(syntaxTreeAvailable(state!, source.length)).toBe(false);
    const parsedTree = ensureSyntaxTree(state!, source.length, 1000);
    expect(parsedTree).not.toBeNull();
    expect(syntaxTreeAvailable(state!, source.length)).toBe(true);
    expect(syntaxTree(state!)).toBe(stateTree);
    expect(parsedTree).not.toBe(stateTree);

    const parent = document.body.appendChild(document.createElement("div"));
    const view = new EditorView({ parent, state: state! });
    views.push(view);
    expect([...view.contentDOM.querySelectorAll<HTMLAnchorElement>("a[data-wiki-page-id]")].map((anchor) => anchor.textContent))
      .toEqual(["Target"]);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("parses target, alias and heading while requiring an exact resolved map entry", () => {
    expect(parseResolvedWikiLinks("[[ Target #walk | Read this ]] [[Target]] [[Missing]]", targets)).toEqual([
      { from: 18, to: 27, tokenFrom: 0, tokenTo: 30, targetLabel: "Target", displayText: "Read this" },
      { from: 33, to: 39, tokenFrom: 31, tokenTo: 41, targetLabel: "Target", displayText: "Target" },
    ]);
  });

  it("keeps markup bytes and exposes only resolved visible text as links outside code, frontmatter, HTML and escapes", () => {
    const source = [
      "---", "title: [[Target]]", "---", "[[Target]]",
      "```md", "[[Target]]", "```", "    [[Target]]",
      "before `[[Target]]` after", "prose <span>[[Target]]</span>",
      "\\[[Target]]", "[[Target|alias]] [[Missing]]",
      "[[Target",
    ].join("\n");
    const { view } = make(source);
    const anchors = [...view.contentDOM.querySelectorAll<HTMLAnchorElement>("a[data-wiki-page-id]")];
    expect(anchors.map((anchor) => anchor.textContent)).toEqual(["Target", "alias"]);
    expect(anchors[0].getAttribute("href")).toBe("#concept:page-target");
    expect(view.contentDOM.textContent).toContain("[[Missing]]");
    expect(view.contentDOM.textContent).toContain("[[Target");
    expect(view.contentDOM.textContent).not.toContain("[[Target|alias]]");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("conceals resolved syntax unless its token is actively edited, including CJK aliases and blur", () => {
    const source = "[[Target|alias]] then [[City Walk#day|城市散步]]";
    const { view } = make(source);
    const firstFrom = source.indexOf("[[Target|alias]]");
    const firstTo = firstFrom + "[[Target|alias]]".length;
    view.dispatch({ selection: { anchor: source.length } });
    view.focus();
    expect(view.contentDOM.textContent).toContain("alias then 城市散步");
    expect(view.contentDOM.textContent).not.toContain("[[Target|alias]]");
    expect(view.contentDOM.textContent).not.toContain("[[City Walk#day|城市散步]]");
    expect(view.state.doc.toString()).toBe(source);

    view.dispatch({ selection: { anchor: firstFrom + 4 } });
    expect(view.contentDOM.textContent).toContain("[[Target|alias]]");
    view.dispatch({ selection: { anchor: firstTo } });
    expect(view.contentDOM.textContent).not.toContain("[[Target|alias]]");

    view.dispatch({ selection: { anchor: firstFrom + 4 } });
    view.contentDOM.blur();
    expect(view.contentDOM.textContent).not.toContain("[[Target|alias]]");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("keeps full resolved syntax stable during composition and reveals it again while editing", () => {
    const source = "[[Target|alias]]";
    const { view } = make(source);
    view.dispatch({ selection: { anchor: 4 } });
    view.focus();
    view.dispatch({ effects: setWritingCompositionActive.of(true) });
    expect(view.contentDOM.textContent).toContain(source);
    expect(view.state.doc.toString()).toBe(source);
    view.dispatch({ effects: setWritingCompositionActive.of(false) });
    expect(view.contentDOM.textContent).toContain(source);
    view.dispatch({ selection: { anchor: source.length } });
    expect(view.contentDOM.textContent).toBe("alias");
    expect(view.state.doc.toString()).toBe(source);
  });

  it("updates resolved links without replacing the editor and preserves source during activation", () => {
    const source = "[[City Walk|two days]]";
    const { view, onActivate } = make(source, new Map());
    expect(view.contentDOM.querySelector("a[data-wiki-page-id]")).toBeNull();
    view.dispatch({ effects: setResolvedWikiLinkTargets(targets) });
    const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
    expect(anchor.dataset.wikiPageId).toBe("page-city");
    anchor.click();
    expect(onActivate).toHaveBeenCalledWith("page-city", anchor);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("keeps the original link target stable through native pointer selection and releases after click", () => {
    const source = "[[Target|alias]]";
    const { view, onActivate } = make(source);
    view.dispatch({ selection: { anchor: source.length } });
    view.focus();
    const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
    const pointerDown = new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 });
    anchor.dispatchEvent(pointerDown);
    view.dispatch({ selection: { anchor: 5 } });
    expect(view.contentDOM.querySelector("a[data-wiki-page-id]")).toBe(anchor);
    expect(view.contentDOM.textContent).not.toContain(source);
    expect(view.state.doc.toString()).toBe(source);

    anchor.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, cancelable: true, button: 0 }));
    const click = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    anchor.dispatchEvent(click);
    expect(click.defaultPrevented).toBe(true);
    expect(onActivate).toHaveBeenCalledWith("page-target", anchor);
    expect(view.state.doc.toString()).toBe(source);
    expect(view.contentDOM.textContent).toContain(source);
  });

  it("preserves a drag selection and removes document pointer listeners when the editor is destroyed", () => {
    const source = "[[Target|alias]]";
    const { view, onActivate } = make(source);
    view.dispatch({ selection: { anchor: source.length } });
    view.focus();
    const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
    const remove = vi.spyOn(document, "removeEventListener");
    anchor.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
    view.dispatch({ selection: { anchor: 5, head: 9 } });
    document.body.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, button: 0 }));
    expect(view.state.selection.main.from).toBe(5);
    expect(view.state.selection.main.to).toBe(9);
    expect(onActivate).not.toHaveBeenCalled();
    expect(view.contentDOM.textContent).toContain("[[Target|alias]]");

    anchor.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
    view.destroy();
    expect(remove).toHaveBeenCalledWith("pointerup", expect.any(Function), true);
    expect(remove).toHaveBeenCalledWith("pointercancel", expect.any(Function), true);
    remove.mockRestore();
    views.splice(views.indexOf(view), 1);
  });

  it("activates the focused link when CodeMirror retargets Enter to its editing host", () => {
    const source = "[[Target|alias]]";
    const { view, onActivate } = make(source);
    const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
    anchor.focus();
    expect(document.activeElement).toBe(anchor);

    // Browser editing hosts can become active again before dispatching keyboard input.
    view.contentDOM.tabIndex = 0;
    view.contentDOM.focus();
    expect(document.activeElement).toBe(anchor);
    const enter = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" });
    (document.activeElement as HTMLElement).dispatchEvent(enter);

    expect(enter.defaultPrevented).toBe(true);
    expect(onActivate).toHaveBeenCalledWith("page-target", anchor);
    expect(view.state.doc.toString()).toBe(source);
  });

  it("leaves modified Enter and reverse Tab to their normal keyboard behavior", () => {
    const { view, onActivate } = make("[[Target|alias]]");
    const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
    anchor.focus();
    const modifiedEnter = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter", ctrlKey: true });
    anchor.dispatchEvent(modifiedEnter);
    expect(modifiedEnter.defaultPrevented).toBe(false);
    expect(onActivate).not.toHaveBeenCalled();

    anchor.focus();
    const reverseTab = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Tab", shiftKey: true });
    anchor.dispatchEvent(reverseTab);
    expect(reverseTab.defaultPrevented).toBe(false);
    expect(onActivate).not.toHaveBeenCalled();
  });

  it("does not reuse link focus after another key, selection movement, text input, or pointerdown", () => {
    const scenarios = ["other-key", "selection", "text-input", "pointerdown"] as const;
    for (const scenario of scenarios) {
      const source = "[[Target|alias]]";
      const { view, onActivate } = make(source);
      const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
      anchor.focus();
      view.contentDOM.tabIndex = 0;
      view.contentDOM.focus();
      expect(document.activeElement).toBe(anchor);

      if (scenario === "other-key") {
        const other = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "x" });
        anchor.dispatchEvent(other);
        expect(other.defaultPrevented).toBe(false);
      } else if (scenario === "selection") {
        view.dispatch({ selection: { anchor: 1 } });
      } else if (scenario === "text-input") {
        view.dispatch({ changes: { from: 0, insert: "x" } });
      } else {
        view.contentDOM.dispatchEvent(new MouseEvent("pointerdown", { bubbles: true, cancelable: true, button: 0 }));
      }

      view.contentDOM.focus();
      const enter = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" });
      view.contentDOM.dispatchEvent(enter);
      expect(enter.defaultPrevented, scenario).toBe(false);
      expect(onActivate, scenario).not.toHaveBeenCalled();
    }
  });

  it("leaves drag selection and Alt-click in editing, never navigates during IME, and opens on focused Enter", () => {
    const { view, onActivate, onPreview, onPreviewLeave } = make("[[Target]]");
    const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-wiki-page-id]")!;
    view.dispatch({ selection: { anchor: 0, head: 2 } });
    const dragClick = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    anchor.dispatchEvent(dragClick);
    expect(dragClick.defaultPrevented).toBe(true);
    expect(onActivate).not.toHaveBeenCalled();
    view.dispatch({ selection: { anchor: 0 } });
    const altClick = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0, altKey: true });
    anchor.dispatchEvent(altClick);
    expect(altClick.defaultPrevented).toBe(true);
    expect(onActivate).not.toHaveBeenCalled();
    anchor.focus();
    const composingEnter = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter", isComposing: true });
    anchor.dispatchEvent(composingEnter);
    expect(onActivate).not.toHaveBeenCalled();
    expect(composingEnter.defaultPrevented).toBe(false);
    const enter = new KeyboardEvent("keydown", { bubbles: true, cancelable: true, key: "Enter" });
    anchor.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    expect(onActivate).toHaveBeenCalledWith("page-target", anchor);
    expect(view.state.doc.toString()).toBe("[[Target]]");
    expect(onPreview).toHaveBeenCalledWith("page-target", anchor, true);
    anchor.dispatchEvent(new FocusEvent("focusout", { bubbles: true, relatedTarget: document.body }));
    expect(onPreviewLeave).toHaveBeenCalledOnce();
  });
});


describe("canonical references share editor behavior", () => {
  it("renders exact page and memory IDs, preserves source, and does not guess external links", () => {
    const source = "[筆記](#concept:p-1) [記憶](#memory:m-1) [網站](https://example.org) ` [code](#memory:m-1)`";
    const { view, onReferenceActivate, onReferencePreview } = make(source);
    const links = [...view.contentDOM.querySelectorAll<HTMLAnchorElement>("a[data-inline-reference-key]")];
    expect(links.map(link => link.textContent)).toEqual(["筆記", "記憶"]);
    expect(links[1].classList.contains("reference-link--memory")).toBe(true);
    links[1].dispatchEvent(new MouseEvent("pointerover", { bubbles: true }));
    expect(onReferencePreview).toHaveBeenCalledWith({ kind: "memory", id: "m-1" }, links[1], false);
    links[1].click();
    expect(onReferenceActivate).toHaveBeenCalledWith({ kind: "memory", id: "m-1" }, links[1]);
    expect(view.state.doc.toString()).toBe(source);
    view.dispatch({ selection: { anchor: source.indexOf("記憶") } });
    view.focus();
    expect(view.contentDOM.textContent).toContain("[記憶](#memory:m-1)");
    view.contentDOM.blur();
    expect(view.contentDOM.textContent).not.toContain("[記憶](#memory:m-1)");
  });

  it("keeps backend citation ordinals across code and removes stale targets when metadata clears", () => {
    const source = "`[1]` content [2] then [3].";
    const citations = [1, 2, 3].map(marker => ({ occurrence: marker, marker, source_kind: marker === 2 ? "memory" as const : "external_file" as const,
      locator: marker === 2 ? "m-2" : "folder::/tmp/demo.md", score: 1, status: marker === 3 ? "unverified" as const : "verified" as const, scope: "sentence" as const }));
    const { view, onReferencePreview } = make(source, targets, { citations });
    const links = [...view.contentDOM.querySelectorAll<HTMLAnchorElement>("a[data-inline-reference-key]")];
    expect(links.map(link => link.textContent)).toEqual(["2", "3"]);
    expect(links[1]).toHaveClass("reference-link--source", "reference-link--unverified");
    expect(links.every(link => link.classList.contains("cm-citation-link"))).toBe(true);
    expect(links[1]).toHaveAttribute("aria-label", "3, source, Unverified");
    links[1].dispatchEvent(new MouseEvent("pointerover", { bubbles: true }));
    expect(onReferencePreview.mock.calls[0][0]).toMatchObject({ kind: "citation", citation: citations[2] });
    view.dispatch({ effects: setEditorReferenceContext({ citations: [] }) });
    expect(view.contentDOM.querySelector("a[data-inline-reference-key]")).toBeNull();
    expect(view.state.doc.toString()).toBe(source);
  });

  it("leaves markers raw when the map mismatches or a marker is escaped, HTML, or frontmatter", () => {
    const citation = { occurrence: 1, marker: 1, source_kind: "memory" as const, locator: "m-1", score: 1, status: "verified" as const, scope: "sentence" as const };
    const { view } = make("[1] [2]", targets, { citations: [citation] });
    expect(view.contentDOM.querySelector("a[data-inline-reference-key]")).toBeNull();
    expect(view.contentDOM.textContent).toContain("[1] [2]");
    for (const source of ["\\[1]", "<span>[1]</span>", "---\nvalue: [1]\n---\nbody"]) {
      const { view: excluded } = make(source, targets, { citations: [citation] });
      expect(excluded.contentDOM.querySelector("a[data-inline-reference-key]")).toBeNull();
    }
  });
});


it("retains touch identity through WebKit's delayed compatibility mouse click", async () => {
  const source = "[memory](#memory:m-1)";
  const { view, onReferenceActivate, onReferencePreview } = make(source);
  const anchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-inline-reference-key]")!;
  const down = new MouseEvent("pointerdown", { bubbles: true, button: 0 });
  Object.defineProperty(down, "pointerType", { value: "touch" });
  anchor.dispatchEvent(down);
  anchor.dispatchEvent(new MouseEvent("pointerup", { bubbles: true, button: 0 }));
  view.contentDOM.dispatchEvent(new FocusEvent("blur"));
  await new Promise(resolve => setTimeout(resolve, 10));
  const finalAnchor = view.contentDOM.querySelector<HTMLAnchorElement>("a[data-inline-reference-key]")!;
  finalAnchor.dispatchEvent(new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 }));
  expect(onReferenceActivate).not.toHaveBeenCalled();
  expect(onReferencePreview).toHaveBeenCalledWith({ kind: "memory", id: "m-1" }, finalAnchor, false);
  expect(view.state.doc.toString()).toBe(source);
});
