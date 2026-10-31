// SPDX-License-Identifier: AGPL-3.0-only
//
// Typing help for the Markdown page editor: Tab and Shift+Tab move list
// items a level in or out, and brackets and quotes close themselves.
//
// Keyboard users leave the editor with Escape, then Tab (or Shift+Tab) within
// two seconds: CodeMirror's tab-focus escape hatch. Ctrl+M (Shift+Alt+M on
// macOS) turns Tab focus moving on until it is pressed again. The host's
// Escape binding sets the two-second window; see CodeMirrorMarkdownEditor.
import { closeBrackets, closeBracketsKeymap } from "@codemirror/autocomplete";
import { indentLess, indentMore, isolateHistory } from "@codemirror/commands";
import { indentUnit, syntaxTree } from "@codemirror/language";
import { EditorSelection, EditorState, type ChangeSpec, type Extension } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";

/**
 * Pairs that close themselves. Single quotes are left out (apostrophes in
 * prose), and so are full-width 「」（）［］: Chinese input methods already
 * offer them as pairs, and pairing them under a live candidate window is
 * where input methods break.
 */
export const AUTO_CLOSED_BRACKETS = ["(", "[", "{", '"', "`"] as const;
/** Typed over a selection, these wrap it instead of replacing it (twice for `**bold**`). */
const WRAPPING_MARKS = new Set(["*", "_"]);

const LIST_LINE = /^([ \t]*)([-*+]|\d{1,9}[.)])([ \t]+|$)/;
const THEMATIC_BREAK = /^[ \t]*([-*_])([ \t]*\1){2,}[ \t]*$/;
const TAB_INDENTED_LIST = /^\t+([-*+]|\d{1,9}[.)])([ \t]|$)/m;
const CODE_NODES = /^(FencedCode|CodeBlock|HTMLBlock|CodeText|CodeInfo)$/;
const TAB_WIDTH = 4;

interface ListLine {
  /** Columns of leading whitespace. */
  indent: number;
  /** Column where the item's text starts: where a child item lines up. */
  content: number;
}

function leadingWhitespace(text: string): string {
  return /^[ \t]*/.exec(text)![0];
}

function columns(whitespace: string): number {
  let column = 0;
  for (const char of whitespace) column = char === "\t" ? column + TAB_WIDTH - (column % TAB_WIDTH) : column + 1;
  return column;
}

function listLine(text: string): ListLine | null {
  const match = LIST_LINE.exec(text);
  if (!match || THEMATIC_BREAK.test(text)) return null;
  const indent = columns(match[1]);
  const markerEnd = indent + match[2].length;
  // A marker with nothing after it, or followed by 5+ spaces, has its text one column on.
  const gap = columns(match[1] + match[2] + match[3]) - markerEnd;
  return { indent, content: markerEnd + (gap >= 1 && gap <= 4 ? gap : 1) };
}

function inCode(state: EditorState, position: number): boolean {
  for (let node: ReturnType<ReturnType<typeof syntaxTree>["resolveInner"]> | null = syntaxTree(state).resolveInner(position, 1); node; node = node.parent) {
    if (CODE_NODES.test(node.name)) return true;
  }
  return false;
}

function listLineAt(state: EditorState, lineNumber: number): ListLine | null {
  const line = state.doc.line(lineNumber);
  const item = listLine(line.text);
  return item && !inCode(state, line.from + leadingWhitespace(line.text).length) ? item : null;
}

/** The last line of the item starting at `lineNumber`: its children and continuation lines. */
function itemEnd(state: EditorState, lineNumber: number, indent: number): number {
  let last = lineNumber;
  for (let next = lineNumber + 1; next <= state.doc.lines; next++) {
    const text = state.doc.line(next).text;
    if (!text.trim()) continue;
    if (columns(leadingWhitespace(text)) <= indent) break;
    last = next;
  }
  return last;
}

/** The nearest earlier line at or left of `indent`, skipping deeper lines and blanks. */
function previousShallowerOrEqual(state: EditorState, lineNumber: number, indent: number, strict: boolean): { line: number; item: ListLine | null; indent: number } | null {
  for (let previous = lineNumber - 1; previous >= 1; previous--) {
    const text = state.doc.line(previous).text;
    if (!text.trim()) continue;
    const at = columns(leadingWhitespace(text));
    if (strict ? at < indent : at <= indent) return { line: previous, item: listLineAt(state, previous), indent: at };
  }
  return null;
}

function reindent(text: string, delta: number, useTabs: boolean): { remove: number; insert: string } {
  const leading = leadingWhitespace(text);
  if (useTabs && Math.abs(delta) === 1) {
    if (delta > 0) return { remove: 0, insert: "\t" };
    if (leading.startsWith("\t")) return { remove: 1, insert: "" };
  }
  const current = columns(leading);
  const target = Math.max(0, current + (useTabs ? delta * TAB_WIDTH : delta));
  if (!leading.includes("\t") && delta > 0) return { remove: 0, insert: " ".repeat(target - current) };
  if (!leading.includes("\t")) return { remove: current - target, insert: "" };
  return { remove: leading.length, insert: "\t".repeat(Math.floor(target / TAB_WIDTH)) + " ".repeat(target % TAB_WIDTH) };
}

function selectedLines(state: EditorState): number[] {
  const lines = new Set<number>();
  for (const range of state.selection.ranges) {
    const first = state.doc.lineAt(range.from).number;
    // A selection ending at the very start of a line does not take that line along.
    let last = state.doc.lineAt(range.to).number;
    if (last > first && range.to === state.doc.line(last).from) last--;
    for (let number = first; number <= last; number++) lines.add(number);
  }
  return [...lines].sort((a, b) => a - b);
}

/**
 * Tab and Shift+Tab on list items. Returns null when no selected line is a
 * list item, so the caller can fall back to plain indentation.
 */
function shiftListItems(state: EditorState, direction: 1 | -1): ChangeSpec[] | null {
  const lines = selectedLines(state);
  const items = lines.map((number) => ({ number, item: listLineAt(state, number) })).filter((entry) => entry.item);
  if (items.length === 0) return null;
  const useTabs = TAB_INDENTED_LIST.test(state.doc.toString());
  // The shallowest selected item decides the step, so the whole selection
  // keeps its shape.
  const lead = items.reduce((best, entry) => entry.item!.indent < best.item!.indent ? entry : best);
  let delta: number;
  if (direction > 0) {
    // An item nests under the sibling above it, lined up with that sibling's text.
    const above = previousShallowerOrEqual(state, lead.number, lead.item!.indent, false);
    if (!above?.item || above.indent !== lead.item!.indent) return [];
    delta = useTabs ? 1 : above.item.content - lead.item!.indent;
  } else {
    if (lead.item!.indent === 0) return [];
    // An item moves out to its parent's column, or to the margin.
    const parent = previousShallowerOrEqual(state, lead.number, lead.item!.indent, true);
    delta = useTabs ? -1 : (parent?.item ? parent.indent : 0) - lead.item!.indent;
  }
  const touched = new Set<number>();
  for (const number of lines) {
    const item = listLineAt(state, number);
    const end = item ? itemEnd(state, number, item.indent) : number;
    for (let line = number; line <= end; line++) touched.add(line);
  }
  const changes: ChangeSpec[] = [];
  for (const number of [...touched].sort((a, b) => a - b)) {
    const line = state.doc.line(number);
    if (!line.text.trim()) continue;
    const { remove, insert } = reindent(line.text, delta, useTabs);
    if (remove || insert) changes.push({ from: line.from, to: line.from + remove, insert });
  }
  return changes;
}

function runShift(view: EditorView, direction: 1 | -1): boolean {
  const { state } = view;
  const changes = shiftListItems(state, direction);
  if (changes === null) return false;
  if (changes.length === 0) return true;
  const changeSet = state.changes(changes);
  view.dispatch({
    changes: changeSet,
    selection: EditorSelection.create(
      state.selection.ranges.map((range) => EditorSelection.range(changeSet.mapPos(range.anchor, 1), changeSet.mapPos(range.head, 1))),
      state.selection.mainIndex,
    ),
    scrollIntoView: true,
    // Each press is its own undo step, even right after typing.
    annotations: isolateHistory.of("full"),
    userEvent: direction > 0 ? "input.indent" : "delete.dedent",
  });
  return true;
}

/** Tab: nest a list item (with its children) under the item above; otherwise insert an indent. */
export function indentMarkdown(view: EditorView): boolean {
  if (view.state.readOnly) return false;
  if (runShift(view, 1)) return true;
  const { state } = view;
  if (state.selection.ranges.some((range) => !range.empty)) return indentMore(view);
  const unit = TAB_INDENTED_LIST.test(state.doc.toString()) ? "\t" : state.facet(indentUnit);
  view.dispatch(state.update(state.replaceSelection(unit), { scrollIntoView: true, userEvent: "input.indent" }));
  return true;
}

/** Shift+Tab: move a list item (with its children) out a level; otherwise remove an indent. */
export function outdentMarkdown(view: EditorView): boolean {
  if (view.state.readOnly) return false;
  if (runShift(view, -1)) return true;
  indentLess(view);
  // Consumed even at the margin, so Shift+Tab never jumps focus by surprise.
  return true;
}

/** Typing `*` or `_` over a selection on one line wraps it instead of replacing it. */
const wrapSelection = () => EditorView.inputHandler.of((view, from, to, text) => {
  if (!WRAPPING_MARKS.has(text) || view.compositionStarted || view.composing || view.state.readOnly) return false;
  const { state } = view;
  const main = state.selection.main;
  if (from !== main.from || to !== main.to) return false;
  if (state.selection.ranges.some((range) => range.empty || state.doc.lineAt(range.from).number !== state.doc.lineAt(range.to).number)) return false;
  view.dispatch(state.update(state.changeByRange((range) => ({
    changes: [{ from: range.from, insert: text }, { from: range.to, insert: text }],
    range: EditorSelection.range(range.anchor + text.length, range.head + text.length),
  })), { scrollIntoView: true, userEvent: "input.type" }));
  return true;
});

/**
 * A third backtick at the start of a line opens a code fence: it becomes
 * exactly three backticks, whatever closers the first two left behind.
 */
const typeFence = () => EditorView.inputHandler.of((view, from, to, text) => {
  if (text !== "`" || from !== to || view.compositionStarted || view.composing || view.state.readOnly) return false;
  const { state } = view;
  if (state.selection.ranges.length !== 1 || !state.selection.main.empty || state.selection.main.head !== from) return false;
  const line = state.doc.lineAt(from);
  if (!/^[ \t]*(?:>[ \t]*)*``$/.test(state.sliceDoc(line.from, from))) return false;
  const after = /^`*/.exec(state.sliceDoc(from, line.to))![0].length;
  view.dispatch(state.update({
    changes: { from, to: from + after, insert: "`" },
    selection: EditorSelection.cursor(from + 1),
    scrollIntoView: true,
    userEvent: "input.type",
  }));
  return true;
});

/**
 * Brackets and quotes close themselves, typing the closer steps over it,
 * Backspace between an empty pair removes both, and typing a bracket, quote,
 * `*` or `_` over a selection wraps it. Nothing pairs while composing.
 */
export function markdownBrackets(): Extension {
  return [
    EditorState.languageData.of(() => [{ closeBrackets: { brackets: [...AUTO_CLOSED_BRACKETS] } }]),
    typeFence(),
    closeBrackets(),
    wrapSelection(),
    keymap.of(closeBracketsKeymap),
  ];
}
