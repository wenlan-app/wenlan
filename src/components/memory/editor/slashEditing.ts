// SPDX-License-Identifier: AGPL-3.0-only
import { isolateHistory } from "@codemirror/commands";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, Prec, StateEffect, StateField, type EditorState, type Extension, type Transaction } from "@codemirror/state";
import { EditorView, keymap, showTooltip, tooltips, type Tooltip } from "@codemirror/view";
import { setWritingCompositionActive } from "./writingPresentation";

// The basic blocks Obsidian's editor offers, in the order people reach for them.
// `keywords` lets "/h3", "/todo" or "/link" find an item in any interface language.
export const slashItems = [
  { id: "heading1", insert: "# ", caret: 2, syntax: "#", keywords: "h1 heading title" },
  { id: "heading2", insert: "## ", caret: 3, syntax: "##", keywords: "h2 heading" },
  { id: "heading3", insert: "### ", caret: 4, syntax: "###", keywords: "h3 heading" },
  { id: "bulletList", insert: "- ", caret: 2, syntax: "-", keywords: "bullet list ul" },
  { id: "numberedList", insert: "1. ", caret: 3, syntax: "1.", keywords: "numbered ordered list ol" },
  { id: "taskList", insert: "- [ ] ", caret: 6, syntax: "- [ ]", keywords: "todo task checkbox" },
  { id: "pageLink", insert: "[", caret: 1, syntax: "[[", keywords: "link page wiki" },
  { id: "blockquote", insert: "> ", caret: 2, syntax: ">", keywords: "quote blockquote" },
  { id: "callout", insert: "> [!note]\n> ", caret: 12, syntax: "> [!]", keywords: "callout note tip warning" },
  { id: "fencedCode", insert: "```\n\n```", caret: 4, syntax: "```", keywords: "code block" },
] as const;
export type SlashItemId = typeof slashItems[number]["id"];
export type SlashLabels = Record<SlashItemId | "label", string>;
export const closeSlashEditing = StateEffect.define<null>();
export const refreshSlashLabels = StateEffect.define<null>();
const selectSlashItem = StateEffect.define<number>();
interface SlashMenu { from: number; to: number; active: number; query: string }
interface SlashState { menu: SlashMenu | null; composing: boolean }
let nextMenuId = 0;

function permittedContext(state: EditorState, position: number): boolean {
  if (state.doc.line(1).text === "---") {
    let end = state.doc.length;
    for (let i = 2; i <= state.doc.lines; i++) {
      const line = state.doc.line(i);
      if (line.text === "---" || line.text === "...") { end = line.to; break; }
    }
    if (position <= end) return false;
  }
  const tree = ensureSyntaxTree(state, position, 20);
  if (!tree) return false;
  for (let node = tree.resolveInner(position, -1); node; node = node.parent!) {
    if (/^(FencedCode|CodeBlock|HTMLBlock)$/.test(node.name)) return false;
  }
  return true;
}

export function typedSlashRange(transaction: Transaction): Omit<SlashMenu, "active" | "query"> | null {
  if (!transaction.isUserEvent("input.type") || transaction.isUserEvent("input.type.compose")) return null;
  const previous = transaction.startState;
  const range = previous.selection.main;
  if (previous.readOnly || previous.selection.ranges.length !== 1 || !range.empty) return null;
  const line = previous.doc.lineAt(range.head);
  if (line.text !== "" || !permittedContext(previous, range.head)) return null;
  let changes = 0;
  let valid = false;
  transaction.changes.iterChanges((from, to, _fromNew, _toNew, inserted) => {
    changes++;
    valid = from === range.head && to === from && inserted.toString() === "/";
  });
  const next = transaction.state.selection;
  if (changes !== 1 || !valid || next.ranges.length !== 1 || !next.main.empty || next.main.head !== range.head + 1) return null;
  return { from: range.head, to: range.head + 1 };
}

/** The items whose label, id or keywords contain the typed query (after "/"). */
export function matchingSlashItems(labels: SlashLabels, query: string): (typeof slashItems)[number][] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [...slashItems];
  return slashItems.filter((item) => `${labels[item.id]} ${item.id} ${item.keywords}`.toLowerCase().includes(needle));
}

/** The slash and what has been typed after it, while the caret stays at its end. */
function menuQuery(state: EditorState, menu: SlashMenu): string | null {
  const selection = state.selection;
  if (state.readOnly || selection.ranges.length !== 1 || !selection.main.empty) return null;
  const line = state.doc.lineAt(menu.from);
  if (line.from !== menu.from || selection.main.head !== line.to) return null;
  const match = /^\/([^\s/]{0,24})$/.exec(line.text);
  return match ? match[1] : null;
}

export function createSlashEditing(options: {
  labels(): SlashLabels;
  blocked(view: EditorView): boolean;
}): { extension: Extension; state: StateField<SlashState> } {
  const menuId = `page-slash-${++nextMenuId}`;
  const state = StateField.define<SlashState>({
    create: () => ({ menu: null, composing: false }),
    update(value, transaction) {
      let { menu, composing } = value;
      let explicitlyClosed = false;
      for (const effect of transaction.effects) {
        if (effect.is(setWritingCompositionActive)) {
          composing = effect.value;
          if (composing) { menu = null; explicitlyClosed = true; }
        }
        if (effect.is(closeSlashEditing)) { menu = null; explicitlyClosed = true; }
        if (effect.is(selectSlashItem) && menu) menu = { ...menu, active: effect.value };
      }
      if (menu && transaction.docChanged) menu = { ...menu, from: transaction.changes.mapPos(menu.from, -1) };
      if (menu && !composing) {
        // Typing after the slash narrows the list; a space, a second slash or
        // no match closes it and leaves the text as typed.
        const query = menuQuery(transaction.state, menu);
        const count = query === null ? 0 : matchingSlashItems(options.labels(), query).length;
        menu = query === null || count === 0 ? null : {
          ...menu,
          to: transaction.state.selection.main.head,
          query,
          active: query === menu.query ? Math.min(menu.active, count - 1) : 0,
        };
      } else if (composing) menu = null;
      if (!menu && !composing && !explicitlyClosed && !transaction.state.readOnly) {
        const range = typedSlashRange(transaction);
        if (range) menu = { ...range, active: 0, query: "" };
      }
      return { menu, composing };
    },
  });
  const close = (view: EditorView): boolean => {
    if (!view.state.field(state).menu) return false;
    view.dispatch({ effects: closeSlashEditing.of(null) });
    return true;
  };
  const accept = (view: EditorView): boolean => {
    const menu = view.state.field(state).menu;
    if (!menu || options.blocked(view) || menuQuery(view.state, menu) === null) return false;
    const item = matchingSlashItems(options.labels(), menu.query)[menu.active];
    if (!item) return false;
    view.dispatch({
      changes: { from: menu.from, to: menu.to, insert: item.insert },
      selection: EditorSelection.cursor(menu.from + item.caret),
      effects: closeSlashEditing.of(null),
      // "Link to page" finishes in a second step below; both undo together.
      annotations: isolateHistory.of(item.id === "pageLink" ? "before" : "full"),
      userEvent: "input.complete",
      scrollIntoView: true,
    });
    // "Link to page" types the second "[" so the page picker opens as if typed.
    if (item.id === "pageLink") {
      const at = view.state.selection.main.head;
      view.dispatch({ changes: { from: at, insert: "[" }, selection: EditorSelection.cursor(at + 1), userEvent: "input.type", annotations: isolateHistory.of("after") });
    }
    view.focus();
    return true;
  };
  const move = (view: EditorView, delta: number): boolean => {
    const menu = view.state.field(state).menu;
    if (!menu || options.blocked(view)) return false;
    const count = matchingSlashItems(options.labels(), menu.query).length;
    view.dispatch({ effects: selectSlashItem.of((menu.active + delta + count) % count) });
    return true;
  };
  const createTooltip = (view: EditorView) => {
      const dom = document.createElement("div");
      dom.id = menuId;
      dom.className = "cm-slash-menu";
      dom.setAttribute("role", "listbox");
      const shown = () => matchingSlashItems(options.labels(), view.state.field(state).menu?.query ?? "");
      const rows = slashItems.map((item) => {
        const row = document.createElement("div");
        const label = document.createElement("span");
        const syntax = document.createElement("span");
        row.id = `${menuId}-${item.id}`;
        row.className = "cm-slash-option";
        row.setAttribute("role", "option");
        syntax.className = "cm-slash-option-syntax";
        syntax.setAttribute("data-syntax", item.syntax);
        syntax.setAttribute("aria-hidden", "true");
        row.append(label, syntax);
        row.addEventListener("pointerenter", () => {
          const menu = view.state.field(state).menu;
          const index = shown().indexOf(item);
          if (!options.blocked(view) && menu && index >= 0 && menu.active !== index) view.dispatch({ effects: selectSlashItem.of(index) });
        });
        row.addEventListener("click", () => {
          const index = shown().indexOf(item);
          if (options.blocked(view) || !view.state.field(state).menu || index < 0) return;
          view.dispatch({ effects: selectSlashItem.of(index) });
          accept(view);
        });
        dom.append(row);
        return { item, row, label };
      });
      const retainFocus = (event: Event) => event.preventDefault();
      dom.addEventListener("pointerdown", retainFocus);
      dom.addEventListener("mousedown", retainFocus);
      const update = () => {
        const labels = options.labels();
        dom.setAttribute("aria-label", labels.label);
        const active = view.state.field(state).menu?.active;
        const visible = shown();
        rows.forEach(({ item, row, label }) => {
          const accessibleLabel = labels[item.id];
          const index = visible.indexOf(item);
          label.textContent = accessibleLabel;
          row.hidden = index < 0;
          row.setAttribute("aria-label", accessibleLabel);
          row.setAttribute("aria-selected", String(index >= 0 && index === active));
        });
      };
      update();
      return { dom, update };
  };
  const tooltip = (value: SlashState): Tooltip | null => value.menu ? {
    pos: value.menu.from + 1,
    above: false,
    create: createTooltip,
  } : null;
  return {
    state,
    extension: [
      state,
      showTooltip.compute([state], (editorState) => tooltip(editorState.field(state))),
      tooltips({ parent: document.body, position: "fixed", tooltipSpace: () => ({ left: 8, right: document.documentElement.clientWidth - 8, top: 60, bottom: window.innerHeight - 8 }) }),
      EditorView.contentAttributes.compute([state], (editorState): Record<string, string> => {
        const menu = editorState.field(state).menu;
        const item = menu ? matchingSlashItems(options.labels(), menu.query)[menu.active] : undefined;
        return menu && item ? { "aria-autocomplete": "list", "aria-controls": menuId, "aria-activedescendant": `${menuId}-${item.id}` } : {};
      }),
      Prec.highest(keymap.of([
        { key: "ArrowDown", run: (view) => move(view, 1) },
        { key: "ArrowUp", run: (view) => move(view, -1) },
        { key: "Enter", run: accept },
        { key: "Escape", run: (view) => options.blocked(view) ? false : close(view) },
        // Tab only closes the menu; it must not also indent the line.
        { key: "Tab", run: (view) => !options.blocked(view) && close(view) },
      ])),
      EditorView.domEventHandlers({ blur: (_event, view) => { close(view); return false; } }),
      EditorView.theme({
        ".cm-slash-menu": { padding: "4px", border: "1px solid var(--mem-border)", borderRadius: "var(--mem-radius-md)", backgroundColor: "var(--mem-detail-surface)", color: "var(--mem-text)", boxShadow: "0 6px 24px rgba(0,0,0,0.12)", width: "220px", maxWidth: "calc(100vw - 16px)", maxHeight: "min(400px, calc(100vh - 76px))", overflowY: "auto", fontFamily: "var(--mem-font-body, system-ui, sans-serif)", fontSize: "var(--mem-text-sm)", zIndex: "1200" },
        ".cm-slash-option[hidden]": { display: "none" },
        ".cm-slash-option": { display: "flex", alignItems: "center", justifyContent: "space-between", gap: "12px", padding: "8px 10px", borderRadius: "var(--mem-radius-sm)", cursor: "pointer" },
        ".cm-slash-option-syntax": { flex: "0 0 auto", fontFamily: "var(--mem-font-mono)", fontSize: "var(--mem-text-label)", color: "var(--mem-text-tertiary)" },
        ".cm-slash-option-syntax::after": { content: "attr(data-syntax)" },
        '.cm-slash-option[aria-selected="true"]': { backgroundColor: "var(--mem-indigo-bg)", color: "var(--mem-accent-page)", outline: "1px solid var(--mem-accent-page)", outlineOffset: "-1px" },
      }),
    ],
  };
}
