// SPDX-License-Identifier: AGPL-3.0-only
import { isolateHistory } from "@codemirror/commands";
import { ensureSyntaxTree } from "@codemirror/language";
import { EditorSelection, Prec, StateEffect, StateField, type EditorState, type Extension, type Transaction } from "@codemirror/state";
import { EditorView, keymap, showTooltip, tooltips, type Tooltip } from "@codemirror/view";
import { linkCompletionText, pageTitleKey, trimLikeDaemon } from "./pageTitleKey";

export { linkCompletionText };

/** A note the writer can link to from the `[[` menu. */
export interface LinkCompletionNote {
  id: string;
  title: string;
  /** Whether the "Use as a source" action is offered for this note. */
  canUseAsSource: boolean;
  /** The note's Space name when it lives in another Space than the page being written. */
  space?: string;
  /** What goes between the brackets: `Space/Title` for a note in another Space, else the title. */
  linkText?: string;
}
export interface LinkCompletionLabels { label: string; empty: string; useAsSource: string; useAsSourceHint: string }
export const closeLinkCompletion = StateEffect.define<null>();
/** Re-render the open menu after the host's notes or labels changed. */
export const refreshLinkCompletion = StateEffect.define<null>();
const selectLinkItem = StateEffect.define<number>();
interface LinkMenu { from: number; to: number; active: number }
interface LinkState { menu: LinkMenu | null; revision: number }
const MAX_ROWS = 8;
const MAX_QUERY = 100;
/** `[` and what Chinese and Japanese input methods type for it. */
const OPENERS = new Set(["[", "\u3010", "\u300c", "\uff3b"]);
const QUERY_STOP = /[[\]|#\n\r\u3010\u3011\u300c\u300d\uff3b\uff3d]/;
let nextMenuId = 0;

/**
 * A title the daemon can resolve from `[[Title]]`: its parser stops at `]`,
 * `|`, `#` and line ends, and drops labels carrying `[`.
 */
export function linkableTitle(title: string): boolean {
  const trimmed = trimLikeDaemon(title);
  return trimmed.length > 0 && !/[[\]|#\n\r]/.test(trimmed);
}

/** How well `key` matches `needle`: 0 a prefix, 1 a word start, 2 anywhere, null no match. */
function matchRank(key: string, needle: string): number | null {
  const at = key.indexOf(needle);
  if (at < 0) return null;
  return at === 0 ? 0 : /[\s\-_/.(]/.test(key[at - 1]) ? 1 : 2;
}

/**
 * The notes shown for `query`: the page's own Space first, then other Spaces,
 * each by prefix matches, then word starts, then any match. A note in another
 * Space matches on its title or on `Space/Title`.
 */
export function linkCompletionMatches(notes: readonly LinkCompletionNote[], query: string): LinkCompletionNote[] {
  const needle = pageTitleKey(query);
  // A link two notes share links to neither, so neither is offered.
  const linkCount = new Map<string, number>();
  for (const note of notes) {
    const key = pageTitleKey(linkCompletionText(note));
    linkCount.set(key, (linkCount.get(key) ?? 0) + 1);
  }
  const ranked: { note: LinkCompletionNote; foreign: number; rank: number; order: number }[] = [];
  notes.forEach((note, order) => {
    const text = linkCompletionText(note);
    if (!linkableTitle(text)) return;
    const key = pageTitleKey(text);
    if (linkCount.get(key) !== 1) return;
    const foreign = note.space === undefined ? 0 : 1;
    if (!needle) { ranked.push({ note, foreign, rank: 0, order }); return; }
    const ranks = [matchRank(pageTitleKey(note.title), needle), matchRank(key, needle)].filter((rank) => rank !== null);
    if (ranks.length === 0) return;
    ranked.push({ note, foreign, rank: Math.min(...ranks), order });
  });
  return ranked
    .sort((a, b) => a.foreign - b.foreign || a.rank - b.rank || a.order - b.order)
    .slice(0, MAX_ROWS)
    .map(({ note }) => note);
}

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
    if (/^(FencedCode|CodeBlock|HTMLBlock|InlineCode)$/.test(node.name)) return false;
  }
  return true;
}

/**
 * The `[[` a writer just finished typing, or null. `「「` and `【【` from a Chinese
 * input method count too. An embed `![[` and a third bracket do not.
 */
export function typedLinkRange(transaction: Transaction): Omit<LinkMenu, "active"> | null {
  if (!transaction.isUserEvent("input.type")) return null;
  const previous = transaction.startState;
  const range = previous.selection.main;
  if (previous.readOnly || previous.selection.ranges.length !== 1 || !range.empty || range.head < 1) return null;
  const opener = previous.sliceDoc(range.head - 1, range.head);
  const before = previous.sliceDoc(Math.max(0, range.head - 2), range.head - 1);
  if (!OPENERS.has(opener) || before === opener || before === "!") return null;
  if (!permittedContext(previous, range.head)) return null;
  let changes = 0;
  let valid = false;
  transaction.changes.iterChanges((from, to, _fromNew, _toNew, inserted) => {
    changes++;
    valid = from === range.head && to === from && inserted.toString() === opener;
  });
  const next = transaction.state.selection;
  if (changes !== 1 || !valid || next.ranges.length !== 1 || !next.main.empty || next.main.head !== range.head + 1) return null;
  return { from: range.head - 1, to: range.head + 1 };
}

function menuQuery(state: EditorState, menu: LinkMenu): string | null {
  const selection = state.selection;
  if (state.readOnly || selection.ranges.length !== 1 || !selection.main.empty || selection.main.head !== menu.to) return null;
  const brackets = state.sliceDoc(menu.from, menu.from + 2);
  if (menu.to < menu.from + 2 || !OPENERS.has(brackets[0]) || brackets[1] !== brackets[0]) return null;
  const query = state.sliceDoc(menu.from + 2, menu.to);
  return query.length <= MAX_QUERY && !QUERY_STOP.test(query) ? query : null;
}

export function createLinkCompletion(options: {
  notes(): readonly LinkCompletionNote[];
  labels(): LinkCompletionLabels;
  blocked(view: EditorView): boolean;
  /** Called after the link is inserted; `asSource` when the writer chose "Use as a source". */
  onPick(note: LinkCompletionNote, asSource: boolean): void;
}): { extension: Extension; state: StateField<LinkState> } {
  const menuId = `page-link-${++nextMenuId}`;
  const state = StateField.define<LinkState>({
    create: () => ({ menu: null, revision: 0 }),
    update(value, transaction) {
      let { menu, revision } = value;
      let explicitlyClosed = false;
      for (const effect of transaction.effects) {
        if (effect.is(closeLinkCompletion)) { menu = null; explicitlyClosed = true; }
        if (effect.is(selectLinkItem) && menu) menu = { ...menu, active: effect.value };
        if (effect.is(refreshLinkCompletion)) revision++;
      }
      // Typing and deleting at the end of the query keep the menu with it,
      // including an input method composing a Chinese title; anything else
      // (a click, an arrow key, an edit elsewhere) closes it.
      if (menu && transaction.docChanged) {
        menu = { from: transaction.changes.mapPos(menu.from, 1), to: transaction.changes.mapPos(menu.to, 1), active: 0 };
      }
      if (menu && menuQuery(transaction.state, menu) === null) menu = null;
      if (!menu && !explicitlyClosed && !transaction.state.readOnly) {
        const range = typedLinkRange(transaction);
        if (range) menu = { ...range, active: 0 };
      }
      return { menu, revision };
    },
  });
  const matchesFor = (editorState: EditorState): LinkCompletionNote[] => {
    const menu = editorState.field(state).menu;
    const query = menu ? menuQuery(editorState, menu) : null;
    return query === null ? [] : linkCompletionMatches(options.notes(), query);
  };
  const activeIndex = (menu: LinkMenu, count: number) => Math.min(menu.active, Math.max(0, count - 1));
  const close = (view: EditorView): boolean => {
    if (!view.state.field(state).menu) return false;
    view.dispatch({ effects: closeLinkCompletion.of(null) });
    return true;
  };
  /** Inserts the active row, or `clicked` when a row was clicked (by id, so a refreshed list cannot swap it). */
  const accept = (view: EditorView, asSource: boolean, clicked?: LinkCompletionNote): boolean => {
    const menu = view.state.field(state).menu;
    if (!menu || options.blocked(view)) return false;
    const matches = matchesFor(view.state);
    if (matches.length === 0) { close(view); return false; }
    const note = clicked ? matches.find((match) => match.id === clicked.id) : matches[activeIndex(menu, matches.length)];
    if (!note) return false;
    const insert = `[[${linkCompletionText(note)}]]`;
    view.dispatch({
      changes: { from: menu.from, to: menu.to, insert },
      selection: EditorSelection.cursor(menu.from + insert.length),
      effects: closeLinkCompletion.of(null),
      annotations: isolateHistory.of("full"),
      userEvent: "input.complete",
      scrollIntoView: true,
    });
    view.focus();
    options.onPick(note, asSource && note.canUseAsSource);
    return true;
  };
  const move = (view: EditorView, delta: number): boolean => {
    const menu = view.state.field(state).menu;
    if (!menu || options.blocked(view)) return false;
    const count = matchesFor(view.state).length;
    if (count === 0) return false;
    view.dispatch({ effects: selectLinkItem.of((activeIndex(menu, count) + delta + count) % count) });
    return true;
  };
  const createTooltip = (view: EditorView) => {
    const dom = document.createElement("div");
    dom.id = menuId;
    dom.className = "cm-link-menu";
    dom.setAttribute("role", "listbox");
    const empty = document.createElement("div");
    empty.className = "cm-link-empty";
    let rows: { row: HTMLElement; note: LinkCompletionNote }[] = [];
    let shown = "";
    // Rows are not rebuilt while a press is in progress, so notes arriving
    // between pointer down and up cannot swallow the click.
    let pressing = false;
    const retainFocus = (event: Event) => event.preventDefault();
    dom.addEventListener("pointerdown", (event) => {
      retainFocus(event);
      pressing = true;
      const release = () => setTimeout(() => { pressing = false; update(); }, 0);
      document.addEventListener("pointerup", release, { once: true, capture: true });
      document.addEventListener("pointercancel", release, { once: true, capture: true });
    });
    dom.addEventListener("mousedown", retainFocus);
    const build = (matches: LinkCompletionNote[], labels: LinkCompletionLabels) => {
      rows = matches.map((note, index) => {
        const row = document.createElement("div");
        const title = document.createElement("span");
        row.id = `${menuId}-${index}`;
        row.className = "cm-link-option";
        row.setAttribute("role", "option");
        row.setAttribute("aria-label", note.space === undefined ? note.title.trim() : linkCompletionText(note));
        title.className = "cm-link-option-title";
        title.textContent = note.title.trim();
        row.append(title);
        if (note.space !== undefined) {
          const space = document.createElement("span");
          title.classList.add("cm-link-option-title-foreign");
          space.className = "cm-link-option-space";
          space.textContent = note.space.trim();
          row.append(space);
        }
        if (note.canUseAsSource) {
          const action = document.createElement("button");
          action.type = "button";
          action.tabIndex = -1;
          action.className = "cm-link-option-action";
          action.textContent = labels.useAsSource;
          action.title = labels.useAsSourceHint;
          action.addEventListener("click", (event) => {
            event.stopPropagation();
            if (options.blocked(view) || !view.state.field(state).menu) return;
            accept(view, true, note);
          });
          row.setAttribute("aria-description", `${labels.useAsSource}: Shift+Enter`);
          row.setAttribute("aria-keyshortcuts", "Shift+Enter");
          row.append(action);
        }
        row.addEventListener("pointerenter", () => {
          const menu = view.state.field(state).menu;
          if (!options.blocked(view) && menu && menu.active !== index) view.dispatch({ effects: selectLinkItem.of(index) });
        });
        row.addEventListener("click", () => {
          if (options.blocked(view) || !view.state.field(state).menu) return;
          accept(view, false, note);
        });
        return { row, note };
      });
      dom.replaceChildren(...(rows.length ? rows.map(({ row }) => row) : [empty]));
    };
    const update = () => {
      const labels = options.labels();
      const matches = matchesFor(view.state);
      dom.setAttribute("aria-label", labels.label);
      empty.textContent = labels.empty;
      const signature = JSON.stringify([labels, matches]);
      if (signature !== shown && !pressing) { shown = signature; build(matches, labels); }
      const menu = view.state.field(state).menu;
      const active = menu ? activeIndex(menu, rows.length) : -1;
      rows.forEach(({ row }, index) => row.setAttribute("aria-selected", String(index === active)));
    };
    update();
    return { dom, update };
  };
  const tooltip = (value: LinkState): Tooltip | null => value.menu ? {
    pos: value.menu.from,
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
        if (!menu) return {};
        const count = matchesFor(editorState).length;
        return {
          "aria-autocomplete": "list",
          "aria-controls": menuId,
          ...(count > 0 ? { "aria-activedescendant": `${menuId}-${activeIndex(menu, count)}` } : {}),
        };
      }),
      Prec.highest(keymap.of([
        { key: "ArrowDown", run: (view) => move(view, 1) },
        { key: "ArrowUp", run: (view) => move(view, -1) },
        { key: "Enter", run: (view) => accept(view, false) },
        { key: "Shift-Enter", run: (view) => accept(view, true) },
        { key: "Escape", run: (view) => options.blocked(view) ? false : close(view) },
        { key: "Tab", run: (view) => { if (!options.blocked(view)) close(view); return false; } },
      ])),
      EditorView.domEventHandlers({ blur: (_event, view) => { close(view); return false; } }),
      EditorView.theme({
        ".cm-link-menu": { padding: "4px", border: "1px solid var(--mem-border)", borderRadius: "var(--mem-radius-md)", backgroundColor: "var(--mem-detail-surface)", color: "var(--mem-text)", boxShadow: "0 6px 24px rgba(0,0,0,0.12)", width: "320px", maxWidth: "calc(100vw - 16px)", maxHeight: "min(320px, calc(100vh - 76px))", overflowY: "auto", fontFamily: "var(--mem-font-body, system-ui, sans-serif)", fontSize: "var(--mem-text-sm)", zIndex: "1200" },
        ".cm-link-option": { display: "flex", alignItems: "center", justifyContent: "space-between", gap: "12px", minHeight: "32px", padding: "4px 6px 4px 10px", borderRadius: "var(--mem-radius-sm)", cursor: "pointer" },
        ".cm-link-option-title": { flex: "1 1 auto", minWidth: "0", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" },
        // A note in another Space keeps its title whole first, then names the Space in the room left.
        ".cm-link-option-title-foreign": { flex: "0 1 auto" },
        ".cm-link-option-space": { flex: "1 1 auto", minWidth: "0", marginLeft: "-4px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", color: "var(--mem-text-tertiary)", fontSize: "var(--mem-text-label)" },
        // Shown on every row it applies to, so a touch screen can reach it; quiet until the row is active.
        ".cm-link-option-action": { flex: "0 0 auto", padding: "2px 8px", border: "1px solid transparent", borderRadius: "var(--mem-radius-sm)", backgroundColor: "transparent", color: "var(--mem-text-tertiary)", font: "inherit", fontSize: "var(--mem-text-label)", cursor: "pointer" },
        ".cm-link-option-action:hover": { color: "var(--mem-accent-page)", borderColor: "var(--mem-accent-page)" },
        '.cm-link-option[aria-selected="true"]': { backgroundColor: "var(--mem-indigo-bg)", color: "var(--mem-accent-page)" },
        '.cm-link-option[aria-selected="true"] .cm-link-option-action': { borderColor: "var(--mem-border)", backgroundColor: "var(--mem-detail-surface)", color: "var(--mem-text-secondary)" },
        ".cm-link-empty": { padding: "8px 10px", color: "var(--mem-text-tertiary)" },
      }),
    ],
  };
}
