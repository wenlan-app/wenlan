// SPDX-License-Identifier: AGPL-3.0-only
import { syntaxTree } from "@codemirror/language";
import { EditorState, Range, StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, WidgetType } from "@codemirror/view";

/**
 * Tables and code blocks read as a table and a block of code, not as their
 * Markdown. A table shows its rows and columns; a code block keeps its lines
 * as written and scrolls sideways instead of wrapping, so a text diagram
 * stays drawn. Clicking either one, or moving the caret into it, shows its
 * Markdown to edit, as the other Markdown constructs do.
 */

const focusChanged = StateEffect.define<boolean>();

const editorFocused = StateField.define<boolean>({
  create: () => false,
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(focusChanged)) value = effect.value;
    return value;
  },
});

type Align = "left" | "center" | "right" | null;

export interface TableBlock {
  kind: "table";
  from: number;
  to: number;
  header: string[];
  align: Align[];
  rows: string[][];
}

export interface CodeBlock {
  kind: "code";
  from: number;
  to: number;
  code: string;
}

export type MarkdownBlock = TableBlock | CodeBlock;

function cellsOf(state: EditorState, row: { from: number; to: number }): string[] {
  const line = state.sliceDoc(row.from, row.to).trim();
  return splitRow(line.replace(/^\|/, "").replace(/(?<!\\)\|$/, "")).map((cell) => cell.trim());
}

/** Splits a row on `|` outside inline code, keeping escaped `\|`. */
function splitRow(row: string): string[] {
  const cells: string[] = [];
  let current = "";
  let inCode = false;
  for (let i = 0; i < row.length; i++) {
    const ch = row[i];
    if (ch === "\\" && row[i + 1] === "|") {
      current += "|";
      i++;
      continue;
    }
    if (ch === "`") inCode = !inCode;
    if (ch === "|" && !inCode) {
      cells.push(current);
      current = "";
      continue;
    }
    current += ch;
  }
  cells.push(current);
  return cells;
}

function alignments(delimiter: string): Align[] {
  const inner = delimiter.trim().replace(/^\|/, "").replace(/\|$/, "");
  return inner.split("|").map((cell) => {
    const spec = cell.trim();
    const left = spec.startsWith(":");
    const right = spec.endsWith(":");
    if (left && right) return "center";
    if (right) return "right";
    if (left) return "left";
    return null;
  });
}

/** Every table and fenced code block in the document, in order. */
export function findMarkdownBlocks(state: EditorState): MarkdownBlock[] {
  const blocks: MarkdownBlock[] = [];
  syntaxTree(state).iterate({
    enter(node) {
      if (node.name === "Table") {
        const header = node.node.getChild("TableHeader");
        const delimiter = node.node.getChild("TableDelimiter");
        if (!header || !delimiter) return false;
        const rows = node.node.getChildren("TableRow").map((row) => cellsOf(state, row));
        blocks.push({
          kind: "table",
          from: state.doc.lineAt(node.from).from,
          to: state.doc.lineAt(Math.max(node.from, node.to - 1)).to,
          header: cellsOf(state, header),
          align: alignments(state.sliceDoc(delimiter.from, delimiter.to)),
          rows,
        });
        return false;
      }
      if (node.name === "FencedCode") {
        const marks = node.node.getChildren("CodeMark");
        const closed = marks.length >= 2;
        const firstLine = state.doc.lineAt(node.from);
        const lastLine = state.doc.lineAt(Math.max(node.from, node.to - 1));
        // An unclosed fence runs to the end of the page while it is typed.
        if (!closed || lastLine.number === firstLine.number) return false;
        const bodyFrom = firstLine.to + 1;
        const bodyTo = lastLine.from - 1;
        blocks.push({
          kind: "code",
          from: firstLine.from,
          to: lastLine.to,
          code: bodyTo > bodyFrom ? state.sliceDoc(bodyFrom, bodyTo) : "",
        });
        return false;
      }
      return undefined;
    },
  });
  return blocks;
}

function escapeHtml(text: string): string {
  return text.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** A table cell's inline Markdown as reading text: code, bold, italic, links. */
export function renderCellHtml(source: string): string {
  const codes: string[] = [];
  let text = source.replace(/`([^`]+)`/g, (_, code: string) => {
    codes.push(`<code>${escapeHtml(code)}</code>`);
    return `\u0000${codes.length - 1}\u0000`;
  });
  text = escapeHtml(text)
    .replace(/&lt;br\s*\/?&gt;/gi, "<br>")
    .replace(/\[\[([^\]|\n]+)\|([^\]\n]+)\]\]/g, (_, _target: string, label: string) => `<span class="cm-md-table-link">${label.trim()}</span>`)
    .replace(/\[\[([^\]\n]+)\]\]/g, (_, target: string) => `<span class="cm-md-table-link">${target.trim()}</span>`)
    .replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, (_, label: string) => `<span class="cm-md-table-link">${label}</span>`)
    .replace(/\*\*([^*]+)\*\*/g, "<strong>$1</strong>")
    .replace(/__([^_]+)__/g, "<strong>$1</strong>")
    .replace(/(^|[^*\w])\*([^*\s][^*]*?)\*(?!\w)/g, "$1<em>$2</em>")
    .replace(/~~([^~]+)~~/g, "<s>$1</s>");
  return text.replace(/\u0000(\d+)\u0000/g, (_, index: string) => codes[Number(index)] ?? "");
}

class TableWidget extends WidgetType {
  constructor(readonly block: TableBlock) {
    super();
  }

  eq(other: TableWidget): boolean {
    return JSON.stringify(other.block) === JSON.stringify(this.block);
  }

  toDOM(view: EditorView): HTMLElement {
    const wrap = document.createElement("div");
    wrap.className = "cm-md-table-wrap";
    const table = document.createElement("table");
    table.className = "cm-md-table";
    const columns = Math.max(this.block.header.length, ...this.block.rows.map((row) => row.length));
    const fill = (row: HTMLTableRowElement, cells: string[], tag: "th" | "td") => {
      for (let i = 0; i < columns; i++) {
        const cell = document.createElement(tag);
        const align = this.block.align[i];
        if (align) cell.style.textAlign = align;
        cell.innerHTML = renderCellHtml(cells[i] ?? "");
        row.appendChild(cell);
      }
    };
    const head = table.createTHead();
    fill(head.insertRow(), this.block.header, "th");
    const body = table.createTBody();
    for (const row of this.block.rows) fill(body.insertRow(), row, "td");
    wrap.appendChild(table);
    wrap.addEventListener("click", (event) => revealBlock(view, wrap, event));
    return wrap;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

class CodeWidget extends WidgetType {
  constructor(readonly block: CodeBlock) {
    super();
  }

  eq(other: CodeWidget): boolean {
    return other.block.code === this.block.code;
  }

  toDOM(view: EditorView): HTMLElement {
    const pre = document.createElement("pre");
    pre.className = "cm-md-code";
    const code = document.createElement("code");
    code.textContent = this.block.code;
    pre.appendChild(code);
    pre.addEventListener("click", (event) => revealBlock(view, pre, event));
    return pre;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

/** Moves the caret into the block, which shows its Markdown to edit. */
function revealBlock(view: EditorView, dom: HTMLElement, event: MouseEvent): void {
  if (event.button !== 0) return;
  // Selecting text in the block to copy it is not a request to edit.
  const selection = dom.ownerDocument.getSelection();
  if (selection && !selection.isCollapsed && dom.contains(selection.anchorNode)) return;
  const pos = view.posAtDOM(dom);
  const line = view.state.doc.lineAt(Math.min(pos, view.state.doc.length));
  view.dispatch({ selection: { anchor: line.from } });
  view.focus();
}

function blockIsRevealed(state: EditorState, block: MarkdownBlock): boolean {
  if (!state.field(editorFocused)) return false;
  return state.selection.ranges.some((range) => range.from <= block.to && range.to >= block.from);
}

function buildBlocks(state: EditorState): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const block of findMarkdownBlocks(state)) {
    if (blockIsRevealed(state, block)) continue;
    const widget = block.kind === "table" ? new TableWidget(block) : new CodeWidget(block);
    ranges.push(Decoration.replace({ widget, block: true }).range(block.from, block.to));
  }
  return Decoration.set(ranges, true);
}

const markdownBlockDecorations = StateField.define<DecorationSet>({
  create: (state) => buildBlocks(state),
  update(value, transaction) {
    const focusChange = transaction.effects.some((effect) => effect.is(focusChanged));
    if (
      transaction.docChanged
      || transaction.selection
      || focusChange
      || syntaxTree(transaction.state) !== syntaxTree(transaction.startState)
    ) {
      return buildBlocks(transaction.state);
    }
    return value;
  },
  provide: (field) => EditorView.decorations.from(field),
});

const markdownBlockTheme = EditorView.theme({
  // A wide table or code line scrolls inside its own box; its width must not
  // widen the page, so its natural width is kept out of the layout.
  ".cm-md-table-wrap, .cm-md-code": {
    width: "100%",
    contain: "inline-size",
  },
  ".cm-md-table-wrap": {
    overflowX: "auto",
    margin: "0.35rem 0 0.6rem",
    cursor: "text",
  },
  ".cm-md-table": {
    borderCollapse: "collapse",
    fontSize: "0.94em",
    lineHeight: "1.55",
    minWidth: "60%",
  },
  ".cm-md-table th, .cm-md-table td": {
    padding: "0.4rem 0.75rem",
    textAlign: "left",
    verticalAlign: "top",
    borderBottom: "1px solid var(--mem-border)",
  },
  ".cm-md-table th": {
    fontWeight: "600",
    color: "var(--mem-text)",
    borderBottomColor: "var(--mem-text-tertiary)",
  },
  ".cm-md-table tr:last-child td": {
    borderBottom: "none",
  },
  ".cm-md-table code, .cm-md-code": {
    fontFamily: "var(--mem-font-mono)",
  },
  ".cm-md-table code": {
    padding: "0.08em 0.3em",
    borderRadius: "3px",
    backgroundColor: "var(--mem-hover)",
    color: "var(--mem-accent-warm)",
  },
  ".cm-md-table-link": {
    color: "var(--mem-link)",
  },
  ".cm-md-code": {
    boxSizing: "border-box",
    margin: "0.35rem 0 0.6rem",
    padding: "0.6rem 0.75rem",
    overflowX: "auto",
    whiteSpace: "pre",
    fontSize: "0.9em",
    lineHeight: "1.6",
    color: "var(--mem-accent-warm)",
    backgroundColor: "var(--mem-hover)",
    borderRadius: "var(--mem-radius-sm)",
    cursor: "text",
  },
});

export function markdownBlocks(): Extension {
  return [
    editorFocused,
    markdownBlockDecorations,
    EditorView.focusChangeEffect.of((_state, focusing) => focusChanged.of(focusing)),
    markdownBlockTheme,
  ];
}
