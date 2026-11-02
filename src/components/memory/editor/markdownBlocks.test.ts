// SPDX-License-Identifier: AGPL-3.0-only
import { afterEach, beforeAll, describe, expect, it } from "vitest";
import { forceParsing } from "@codemirror/language";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { findMarkdownBlocks, markdownBlocks, renderCellHtml } from "./markdownBlocks";
import { installCodeMirrorDomPolyfills, selectRange } from "./editorTestUtils";

beforeAll(installCodeMirrorDomPolyfills);
const views: EditorView[] = [];
afterEach(() => { views.splice(0).forEach((view) => { const parent = view.dom.parentElement; view.destroy(); parent?.remove(); }); });

function make(source: string) {
  const parent = document.body.appendChild(document.createElement("div"));
  const view = new EditorView({
    parent,
    state: EditorState.create({ doc: source, extensions: [markdown({ base: markdownLanguage }), markdownBlocks()] }),
  });
  views.push(view);
  expect(forceParsing(view, source.length, 1000)).toBe(true);
  view.dispatch({});
  return view;
}

const TABLE = ["Intro", "", "| Rank | Note |", "|---|:---:|", "| 1 | **full** weight |", "| 3 | `a \\| b` |", "", "After"].join("\n");
const CODE = ["Intro", "", "```", "INTENT → WIREFRAME", "  brief    sketch", "```", "", "After"].join("\n");

describe("markdown blocks", () => {
  it("reads a table's header, alignment and rows, keeping a pipe inside code", () => {
    const view = make(TABLE);
    const [table] = findMarkdownBlocks(view.state);
    expect(table).toMatchObject({ kind: "table", header: ["Rank", "Note"], align: [null, "center"], rows: [["1", "**full** weight"], ["3", "`a | b`"]] });
  });

  it("shows a table as a table and a code block as its lines, not their Markdown", () => {
    const view = make(`${TABLE}\n\n${CODE}`);
    const table = view.dom.querySelector("table.cm-md-table");
    expect(table?.querySelectorAll("th")).toHaveLength(2);
    expect(table?.querySelector("td strong")?.textContent).toBe("full");
    expect(view.contentDOM.textContent).not.toContain("|---|");
    const code = view.dom.querySelector("pre.cm-md-code");
    expect(code?.textContent).toBe("INTENT → WIREFRAME\n  brief    sketch");
    expect(view.contentDOM.textContent).not.toContain("```");
  });

  it("shows the Markdown again while the caret is inside the block", async () => {
    const view = make(TABLE);
    view.focus();
    await new Promise((resolve) => setTimeout(resolve, 30));
    selectRange(view, TABLE.indexOf("| 1"));
    expect(view.dom.querySelector("table.cm-md-table")).toBeNull();
    expect(view.contentDOM.textContent).toContain("|---|:---:|");
  });

  it("leaves an unclosed code fence as text while it is typed", () => {
    const view = make("```\nstill typing");
    expect(findMarkdownBlocks(view.state)).toEqual([]);
  });

  it("renders a cell's inline Markdown as text, escaping HTML", () => {
    expect(renderCellHtml("**a** <b> [[Page|label]] [site](https://x.y) `<i>`")).toBe(
      '<strong>a</strong> &lt;b&gt; <span class="cm-md-table-link">label</span> <span class="cm-md-table-link">site</span> <code>&lt;i&gt;</code>',
    );
  });
});
