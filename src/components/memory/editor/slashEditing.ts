// SPDX-License-Identifier: AGPL-3.0-only
import { isolateHistory } from "@codemirror/commands";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, Prec, StateEffect, StateField, type EditorState, type Extension, type Transaction } from "@codemirror/state";
import { EditorView, keymap, showTooltip, tooltips, type Tooltip } from "@codemirror/view";
import { setWritingCompositionActive } from "./writingPresentation";

export const slashItems = [
  { id: "heading1", insert: "# ", caret: 2, syntax: "#" },
  { id: "heading2", insert: "## ", caret: 3, syntax: "##" },
  { id: "bulletList", insert: "- ", caret: 2, syntax: "-" },
  { id: "numberedList", insert: "1. ", caret: 3, syntax: "1." },
  { id: "taskList", insert: "- [ ] ", caret: 6, syntax: "- [ ]" },
  { id: "blockquote", insert: "> ", caret: 2, syntax: ">" },
  { id: "fencedCode", insert: "```\n\n```", caret: 4, syntax: "```" },
] as const;
export type SlashItemId = typeof slashItems[number]["id"];
export type SlashLabels = Record<SlashItemId | "label", string>;
export const closeSlashEditing = StateEffect.define<null>();
export const refreshSlashLabels = StateEffect.define<null>();
const selectSlashItem = StateEffect.define<number>();
interface SlashMenu { from: number; to: number; active: number }
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

export function typedSlashRange(transaction: Transaction): Omit<SlashMenu, "active"> | null {
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

function validMenu(state: EditorState, menu: SlashMenu): boolean {
  const selection = state.selection;
  return !state.readOnly && selection.ranges.length === 1 && selection.main.empty
    && selection.main.head === menu.to && state.doc.lineAt(menu.from).text === "/"
    && state.sliceDoc(menu.from, menu.to) === "/";
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
      if (menu && (composing || !validMenu(transaction.state, menu))) menu = null;
      if (!menu && !composing && !explicitlyClosed && !transaction.state.readOnly) {
        const range = typedSlashRange(transaction);
        if (range) menu = { ...range, active: 0 };
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
    if (!menu || options.blocked(view) || !validMenu(view.state, menu)) return false;
    const item = slashItems[menu.active];
    view.dispatch({
      changes: { from: menu.from, to: menu.to, insert: item.insert },
      selection: EditorSelection.cursor(menu.from + item.caret),
      effects: closeSlashEditing.of(null),
      annotations: isolateHistory.of("full"),
      userEvent: "input.complete",
      scrollIntoView: true,
    });
    view.focus();
    return true;
  };
  const move = (view: EditorView, delta: number): boolean => {
    const menu = view.state.field(state).menu;
    if (!menu || options.blocked(view)) return false;
    view.dispatch({ effects: selectSlashItem.of((menu.active + delta + slashItems.length) % slashItems.length) });
    return true;
  };
  const createTooltip = (view: EditorView) => {
      const dom = document.createElement("div");
      dom.id = menuId;
      dom.className = "cm-slash-menu";
      dom.setAttribute("role", "listbox");
      const rows = slashItems.map((item, index) => {
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
          if (!options.blocked(view) && menu && menu.active !== index) view.dispatch({ effects: selectSlashItem.of(index) });
        });
        row.addEventListener("click", () => {
          if (options.blocked(view) || !view.state.field(state).menu) return;
          view.dispatch({ effects: selectSlashItem.of(index) });
          accept(view);
        });
        dom.append(row);
        return { row, label };
      });
      const retainFocus = (event: Event) => event.preventDefault();
      dom.addEventListener("pointerdown", retainFocus);
      dom.addEventListener("mousedown", retainFocus);
      const update = () => {
        const labels = options.labels();
        dom.setAttribute("aria-label", labels.label);
        const active = view.state.field(state).menu?.active;
        rows.forEach(({ row, label }, index) => {
          const accessibleLabel = labels[slashItems[index].id];
          label.textContent = accessibleLabel;
          row.setAttribute("aria-label", accessibleLabel);
          row.setAttribute("aria-selected", String(index === active));
        });
      };
      update();
      return { dom, update };
  };
  const tooltip = (value: SlashState): Tooltip | null => value.menu ? {
    pos: value.menu.to,
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
        return menu ? { "aria-autocomplete": "list", "aria-controls": menuId, "aria-activedescendant": `${menuId}-${slashItems[menu.active].id}` } : {};
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
        ".cm-slash-menu": { padding: "4px", border: "1px solid var(--mem-border)", borderRadius: "var(--mem-radius-md)", backgroundColor: "var(--mem-detail-surface)", color: "var(--mem-text)", boxShadow: "0 6px 24px rgba(0,0,0,0.12)", width: "220px", maxWidth: "calc(100vw - 16px)", maxHeight: "min(320px, calc(100vh - 76px))", overflowY: "auto", fontFamily: "var(--mem-font-body, system-ui, sans-serif)", fontSize: "var(--mem-text-sm)", zIndex: "1200" },
        ".cm-slash-option": { display: "flex", alignItems: "center", justifyContent: "space-between", gap: "12px", padding: "8px 10px", borderRadius: "var(--mem-radius-sm)", cursor: "pointer" },
        ".cm-slash-option-syntax": { flex: "0 0 auto", fontFamily: "var(--mem-font-mono)", fontSize: "var(--mem-text-label)", color: "var(--mem-text-tertiary)" },
        ".cm-slash-option-syntax::after": { content: "attr(data-syntax)" },
        '.cm-slash-option[aria-selected="true"]': { backgroundColor: "var(--mem-indigo-bg)", color: "var(--mem-accent-page)", outline: "1px solid var(--mem-accent-page)", outlineOffset: "-1px" },
      }),
    ],
  };
}
