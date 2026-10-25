// SPDX-License-Identifier: AGPL-3.0-only
import { syntaxTree } from "@codemirror/language";
import { Annotation, Facet, Prec, StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap, ViewPlugin, WidgetType } from "@codemirror/view";
import { sourceLinkKey } from "./sourceLinkKey";

type SyntaxNode = ReturnType<typeof syntaxTree>["topNode"];

/** Where a saved link sits, and whether it shows as a preview card. */
export interface SourceLinkPlace {
  /** Alone on its line, with a source that has words to show. */
  canPreview: boolean;
  preview: boolean;
  setPreview(on: boolean): void;
  /** Tells the editor a card changed height, as a post's embed does once it shows. */
  resized?(): void;
}

/** Draws a link's site icon into `container`; returns its cleanup. */
export type SourceIconMount = (container: HTMLElement, url: string, place?: SourceLinkPlace) => () => void;
/** Draws a link's preview card into `container`; returns its cleanup. */
export type SourceCardMount = (container: HTMLElement, url: string, title: string, place: SourceLinkPlace) => () => void;

/** A saved source, as its links show it. */
export interface SourceLinkInfo {
  title: string;
  /** Its source has words a preview card can show. */
  previewable: boolean;
}

export interface PastedSourceSaved {
  title: string;
  /** Only the link and its title were saved, because the site needs a
   *  sign-in or its page could not be read. */
  linkOnly?: "signIn" | "unreadable";
  /** Deletes the source again; set only when this paste created it. */
  remove?(): Promise<void>;
}

export interface SourcePasteLabels {
  saving: string;
  savedLinkOnly: string;
  unreadable: string;
  failed: string;
  retry: string;
}

export interface SourcePasteOptions {
  blocked(view: EditorView): boolean;
  /** Whether a pasted link may be saved as a source right now. */
  enabled(): boolean;
  labels(): SourcePasteLabels;
  /** `label` is the text the link was pasted over, the writer's own name for it. */
  save(url: string, label?: string): Promise<PastedSourceSaved>;
}

/** The links already saved as sources; they show their site icon. */
export const setSourceLinkUrls = StateEffect.define<ReadonlyMap<string, SourceLinkInfo>>();
export const sourceLinkUrls = StateField.define<ReadonlyMap<string, SourceLinkInfo>>({
  create: () => new Map(),
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setSourceLinkUrls)) value = effect.value;
    return value;
  },
});

/** A link that just took its title. It reads as a link while the caret stays
 *  beside it, instead of opening into Markdown at once. */
const setFreshLink = StateEffect.define<{ from: number; to: number } | null>();
const freshLink = StateField.define<{ from: number; to: number } | null>({
  create: () => null,
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setFreshLink)) return effect.value;
    if (!value) return value;
    const from = transaction.changes.mapPos(value.from, 1);
    const to = transaction.changes.mapPos(value.to, -1);
    if (from >= to) return null;
    const head = transaction.state.selection.main.head;
    const doc = transaction.state.doc;
    // Into the link, or off its line: from then on it opens like any link.
    if ((head > from && head < to) || doc.lineAt(head).number !== doc.lineAt(from).number) return null;
    return from === value.from && to === value.to ? value : { from, to };
  },
});

/** The source a link points at, also when only its tracking parameters,
 *  fragment or trailing slash differ. */
export function sourceLinkInfo(links: ReadonlyMap<string, SourceLinkInfo> | undefined, url: string): SourceLinkInfo | undefined {
  if (!links) return undefined;
  const exact = links.get(url);
  if (exact !== undefined) return exact;
  const key = sourceLinkKey(url);
  for (const [link, info] of links) if (sourceLinkKey(link) === key) return info;
  return undefined;
}

/** The link title that marks a link shown as a preview card: `[name](url "preview")`. */
const PREVIEW_TITLE = '"preview"';

/** Remembers whether links show as preview cards, so the next paste follows the writer's last choice. */
export interface SourceLinkPreference {
  prefer(): boolean;
  remember(preview: boolean): void;
}

export const sourceLinkPreference = Facet.define<SourceLinkPreference, SourceLinkPreference | null>({
  combine: (values) => values[0] ?? null,
});

export const sourceCardMount = Facet.define<SourceCardMount, SourceCardMount | null>({
  combine: (values) => values[0] ?? null,
});

/** A link's own parts, read from the Markdown at `from`. */
function linkAt(state: EditorState, from: number): { from: number; to: number; label: string; url: string; preview: boolean } | null {
  for (let node: SyntaxNode | null = syntaxTree(state).resolveInner(from + 1, 1); node; node = node.parent) {
    if (node.name !== "Link") continue;
    const marks = node.getChildren("LinkMark");
    const url = node.getChild("URL");
    if (!url || marks.length < 4) return null;
    const title = node.getChild("LinkTitle");
    return {
      from: node.from,
      to: node.to,
      label: state.sliceDoc(marks[0].to, marks[1].from),
      url: state.sliceDoc(url.from, url.to),
      preview: Boolean(title && state.sliceDoc(title.from, title.to) === PREVIEW_TITLE),
    };
  }
  return null;
}

/** Whether the link at `from`..`to` is alone on its line, and marked to show as a preview card. */
export function sourceLinkPlaceAt(state: EditorState, from: number, to: number): { alone: boolean; preview: boolean } {
  const line = state.doc.lineAt(from);
  const title = linkAt(state, from);
  return {
    alone: to <= line.to && line.text.trim() === state.sliceDoc(from, to),
    preview: Boolean(title?.preview),
  };
}

/** Shows the link at `from` as a preview card, or back as a link, and remembers the choice. */
export function setSourceLinkPreview(view: EditorView, from: number, preview: boolean): void {
  const link = linkAt(view.state, from);
  if (!link || link.preview === preview || view.state.readOnly) return;
  const insert = `[${link.label}](${link.url}${preview ? ` ${PREVIEW_TITLE}` : ""})`;
  view.dispatch({ changes: { from: link.from, to: link.to, insert }, userEvent: "input" });
  view.state.facet(sourceLinkPreference)?.remember(preview);
}

function linkPlace(view: EditorView, from: number | (() => number), canPreview: boolean, preview: boolean): SourceLinkPlace {
  const at = typeof from === "number" ? () => from : from;
  return {
    // A page that cannot be edited keeps its links as they are.
    canPreview: canPreview && !view.state.readOnly,
    preview,
    setPreview: (on) => setSourceLinkPreview(view, at(), on),
    resized: () => view.requestMeasure(),
  };
}

/** Whether the link at `from`..`to` just took its title and stays closed. */
export function sourceLinkIsFresh(state: EditorState, from: number, to: number): boolean {
  const fresh = state.field(freshLink, false);
  return Boolean(fresh && fresh.from === from && fresh.to === to);
}

export const sourceIconMount = Facet.define<SourceIconMount, SourceIconMount | null>({
  combine: (values) => values[0] ?? null,
});

const iconCleanups = new WeakMap<HTMLElement, () => void>();

function mountIcon(view: EditorView, container: HTMLElement, url: string, place?: SourceLinkPlace): void {
  const mount = view.state.facet(sourceIconMount);
  if (mount) iconCleanups.set(container, mount(container, url, place));
}

function unmountIcon(container: HTMLElement): void {
  iconCleanups.get(container)?.();
  iconCleanups.delete(container);
}

/** The site icon after a link saved as a source, where other links show an arrow. */
export class SourceLinkIconWidget extends WidgetType {
  constructor(readonly url: string, readonly from = -1, readonly canPreview = false, readonly preview = false) {
    super();
  }

  eq(other: SourceLinkIconWidget): boolean {
    return other.url === this.url && other.from === this.from && other.canPreview === this.canPreview && other.preview === this.preview;
  }

  toDOM(view: EditorView): HTMLElement {
    const icon = document.createElement("span");
    icon.className = "cm-source-link-icon";
    // The mount draws the icon, and a button to open the source when it can.
    mountIcon(view, icon, this.url, this.from < 0 ? undefined : linkPlace(view, this.from, this.canPreview, this.preview));
    return icon;
  }

  destroy(dom: HTMLElement): void {
    unmountIcon(dom);
  }
}

/** A link shown as a preview card of its source, in place of its line. */
export class SourceLinkCardWidget extends WidgetType {
  constructor(readonly url: string, readonly title: string, readonly from: number) {
    super();
  }

  eq(other: SourceLinkCardWidget): boolean {
    return other.url === this.url && other.title === this.title && other.from === this.from;
  }

  toDOM(view: EditorView): HTMLElement {
    const card = document.createElement("span");
    card.className = "cm-source-link-card";
    const spot = { from: this.from };
    cardSpots.set(card, spot);
    const mount = view.state.facet(sourceCardMount);
    if (mount) iconCleanups.set(card, mount(card, this.url, this.title, linkPlace(view, () => spot.from, true, true)));
    return card;
  }

  // Text typed above a card only moves it: the card, and a video playing in
  // it, stays as it is.
  updateDOM(dom: HTMLElement, _view: EditorView, previous: SourceLinkCardWidget): boolean {
    const spot = cardSpots.get(dom);
    if (!spot || previous.url !== this.url || previous.title !== this.title) return false;
    spot.from = this.from;
    return true;
  }

  destroy(dom: HTMLElement): void {
    unmountIcon(dom);
    cardSpots.delete(dom);
  }
}

const cardSpots = new WeakMap<HTMLElement, { from: number }>();

/** A link worth offering as a source: one public http(s) address, nothing else. */
export function pastedSourceUrl(text: string): string | null {
  const value = text.trim();
  if (!/^https?:\/\/\S+$/i.test(value)) return null;
  try {
    const url = new URL(value);
    if (url.username || url.password || !url.hostname.includes(".")) return null;
    return value;
  } catch {
    return null;
  }
}

/** Markdown for a link with its page title as the text. */
export function sourceLinkMarkdown(title: string, url: string, preview = false): string {
  const text = title.replace(/\s+/g, " ").trim().slice(0, 140).replace(/[\\[\]]/g, "\\$&");
  return text ? `[${text}](${url}${preview ? ` ${PREVIEW_TITLE}` : ""})` : url;
}

/** Whether a link put at `from`..`to` will be alone on its line and should follow the writer's preview choice. */
function showsAsCard(state: EditorState, from: number, to: number, previewable: boolean): boolean {
  if (!previewable || !state.facet(sourceLinkPreference)?.prefer()) return false;
  return !state.sliceDoc(state.doc.lineAt(from).from, from).trim() && !state.sliceDoc(to, state.doc.lineAt(to).to).trim();
}

type PromptPhase = "saving" | "linkOnly" | "failed";

interface Prompt {
  id: number;
  from: number;
  to: number;
  url: string;
  label?: string;
  phase: PromptPhase;
  linkOnly?: PastedSourceSaved["linkOnly"];
}

interface PromptState {
  prompt: Prompt | null;
  /** Links already saved in this editing session; each is saved once. */
  offered: ReadonlySet<string>;
}

const setPrompt = StateEffect.define<Prompt | null>();
/** A link whose source was deleted with it; pasting it again saves it again. */
const forgetLink = StateEffect.define<string>();
const ownChange = Annotation.define<true>();
let nextPromptId = 0;
/** Sources a paste created, by editor and link, until their link is deleted. */
const createdSources = new WeakMap<EditorView, Map<string, () => Promise<void>>>();
/** How long a deleted link stays deleted before its source goes too, so a cut and paste keeps it. */
const REMOVE_AFTER_MS = 3000;

const promptState: StateField<PromptState> = StateField.define<PromptState>({
  create: () => ({ prompt: null, offered: new Set() }),
  update(value, transaction) {
    let { prompt, offered } = value;
    if (prompt && transaction.docChanged) {
      // Writing on puts the note away; only a save in flight keeps it.
      if (prompt.phase !== "saving" && !transaction.annotation(ownChange)) prompt = null;
      else prompt = { ...prompt, from: transaction.changes.mapPos(prompt.from, 1), to: transaction.changes.mapPos(prompt.to, -1) };
    }
    for (const effect of transaction.effects) {
      if (effect.is(forgetLink) && offered.has(effect.value)) {
        offered = new Set([...offered].filter((url) => url !== effect.value));
      }
      if (!effect.is(setPrompt)) continue;
      prompt = effect.value;
      if (prompt && !offered.has(prompt.url)) offered = new Set([...offered, prompt.url]);
    }
    return prompt === value.prompt && offered === value.offered ? value : { prompt, offered };
  },
});

// Built when the extension is, not at import: the note is a block widget, so
// it must come from state rather than a view plugin.
function promptDecorations(): Extension {
  return EditorView.decorations.compute([promptState], (state) => {
    const { prompt } = state.field(promptState);
    if (!prompt || prompt.to > state.doc.length) return Decoration.none;
    // Saving turns where the icon will be, so nothing moves.
    if (prompt.phase === "saving") {
      return Decoration.set([Decoration.widget({ widget: new SavingWidget(), side: 1 }).range(prompt.to)]);
    }
    const at = state.doc.lineAt(prompt.to).to;
    return Decoration.set([
      Decoration.widget({ widget: new PromptWidget(prompt), block: true, side: 1 }).range(at),
    ]);
  });
}

class SavingWidget extends WidgetType {
  eq(): boolean {
    return true;
  }

  toDOM(view: EditorView): HTMLElement {
    const spinner = document.createElement("span");
    spinner.className = "cm-source-saving";
    spinner.setAttribute("role", "status");
    const label = view.state.facet(promptOptions)?.labels().saving ?? "";
    spinner.setAttribute("aria-label", label);
    spinner.title = label;
    return spinner;
  }
}

const promptOptions = Facet.define<SourcePasteOptions, SourcePasteOptions | null>({
  combine: (values) => values[0] ?? null,
});

function currentPrompt(state: EditorState, id: number): Prompt | null {
  const prompt = state.field(promptState, false)?.prompt ?? null;
  return prompt?.id === id ? prompt : null;
}

function update(view: EditorView, id: number, phase: PromptPhase | null): void {
  const prompt = currentPrompt(view.state, id);
  if (!prompt) return;
  view.dispatch({ effects: setPrompt.of(phase ? { ...prompt, phase } : null) });
}

function save(view: EditorView, id: number): void {
  const options = view.state.facet(promptOptions);
  const prompt = currentPrompt(view.state, id);
  if (!options || !prompt || options.blocked(view)) return;
  if (prompt.phase !== "saving") update(view, id, "saving");
  void options.save(prompt.url, prompt.label).then(
    (saved) => {
      if (!view.dom.isConnected) return;
      const current = currentPrompt(view.state, id);
      if (!current) return;
      if (saved.remove) {
        const created = createdSources.get(view) ?? new Map<string, () => Promise<void>>();
        created.set(current.url, saved.remove);
        createdSources.set(view, created);
      }
      // Saved is shown by the site icon alone; a note stays only for a link
      // saved without its page.
      const next = saved.linkOnly ? { ...current, phase: "linkOnly" as const, linkOnly: saved.linkOnly } : null;
      const markdown = sourceLinkMarkdown(saved.title, current.url, showsAsCard(view.state, current.from, current.to, !saved.linkOnly));
      // The link reads as its title once saved, unless the writer changed it meanwhile.
      if (view.state.sliceDoc(current.from, current.to) === current.url && markdown !== current.url && !view.state.readOnly) {
        const to = current.from + markdown.length;
        view.dispatch({
          changes: { from: current.from, to: current.to, insert: markdown },
          effects: [setPrompt.of(next && { ...next, to }), setFreshLink.of({ from: current.from, to })],
          annotations: ownChange.of(true),
          userEvent: "input.paste",
        });
      } else {
        view.dispatch({ effects: setPrompt.of(next) });
      }
      if (next) {
        window.setTimeout(() => {
          if (view.dom.isConnected && currentPrompt(view.state, id)?.phase === "linkOnly") update(view, id, null);
        }, 5000);
      }
    },
    () => {
      if (view.dom.isConnected) update(view, id, "failed");
    },
  );
}

class PromptWidget extends WidgetType {
  constructor(readonly prompt: Prompt) {
    super();
  }

  eq(other: PromptWidget): boolean {
    return other.prompt.id === this.prompt.id && other.prompt.phase === this.prompt.phase && other.prompt.linkOnly === this.prompt.linkOnly && other.prompt.url === this.prompt.url;
  }

  toDOM(view: EditorView): HTMLElement {
    const labels = view.state.facet(promptOptions)?.labels();
    const row = document.createElement("div");
    row.className = "cm-source-paste";
    row.dataset.phase = this.prompt.phase;
    if (!labels) return row;
    const { id, phase } = this.prompt;
    const button = (text: string, onClick: () => void): HTMLButtonElement => {
      const element = document.createElement("button");
      element.type = "button";
      element.textContent = text;
      // Keep the caret where it is: the click is not a move into the widget.
      element.addEventListener("mousedown", (event) => event.preventDefault());
      element.addEventListener("click", (event) => {
        event.preventDefault();
        onClick();
      });
      return element;
    };
    const status = (text: string): HTMLElement => {
      const element = document.createElement("span");
      element.className = "cm-source-paste-status";
      element.setAttribute("role", "status");
      element.textContent = text;
      return element;
    };
    if (phase === "saving") {
      row.append(status(labels.saving));
    } else if (phase === "failed") {
      row.append(status(labels.failed), button(labels.retry, () => save(view, id)));
    } else {
      row.append(status(this.prompt.linkOnly === "unreadable" ? labels.unreadable : labels.savedLinkOnly));
    }
    return row;
  }

  ignoreEvent(): boolean {
    return true;
  }
}

/** Deleting a pasted link deletes the source that paste created. */
const removeWithLink = ViewPlugin.fromClass(class {
  private timer: number | null = null;

  constructor(private readonly view: EditorView) {}

  update(update: { docChanged: boolean }): void {
    if (!update.docChanged || !createdSources.get(this.view)?.size) return;
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => {
      this.timer = null;
      this.sweep();
    }, REMOVE_AFTER_MS);
  }

  destroy(): void {
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.sweep(false);
  }

  private sweep(live = true): void {
    const created = createdSources.get(this.view);
    if (!created) return;
    const text = this.view.state.doc.toString();
    const kept = new Set([...text.matchAll(/https?:\/\/[^\s<>()[\]]+/g)].map((match) => sourceLinkKey(match[0])));
    for (const [url, remove] of [...created]) {
      // Trimming tracking parameters or a trailing slash still links the same page.
      if (text.includes(url) || kept.has(sourceLinkKey(url))) continue;
      created.delete(url);
      void remove().catch(() => undefined);
      if (live) this.view.dispatch({ effects: forgetLink.of(url) });
    }
  }
});

/** Code and link destinations are not places to save a source from. */
function pasteTargetsProse(state: EditorState, at: number): boolean {
  const before = state.sliceDoc(Math.max(0, at - 1), at);
  if (before === "(" || before === "<" || before === "[") return false;
  const inner = syntaxTree(state).resolveInner(at, -1);
  for (let node: typeof inner | null = inner; node; node = node.parent) {
    if (/Code|Link|URL|HTMLBlock|Comment/.test(node.name)) return false;
  }
  return true;
}

const theme = EditorView.theme({
  ".cm-source-paste": {
    display: "flex",
    alignItems: "center",
    gap: "2px",
    width: "fit-content",
    margin: "0.2rem 0 0.4rem",
    fontFamily: "var(--mem-font-body)",
    fontSize: "13px",
    lineHeight: "1.4",
    color: "var(--mem-text-secondary)",
    whiteSpace: "nowrap",
  },
  ".cm-source-paste button": {
    marginLeft: "6px",
    padding: "3px 6px",
    border: "0",
    borderRadius: "6px",
    font: "inherit",
    color: "var(--mem-link)",
    backgroundColor: "transparent",
    cursor: "pointer",
  },
  ".cm-source-paste button:hover": {
    backgroundColor: "var(--mem-hover)",
  },
  ".cm-source-paste button:focus-visible": {
    outline: "2px solid var(--mem-accent-indigo)",
    outlineOffset: "1px",
  },
  ".cm-source-paste-status": {
    padding: "3px 0",
  },
  ".cm-source-link-icon": {
    display: "inline-flex",
    alignItems: "center",
    width: "14px",
    height: "14px",
    marginLeft: "0.2em",
    verticalAlign: "-0.12em",
    color: "var(--mem-text-tertiary)",
  },
  ".cm-source-link-icon button": {
    display: "inline-flex",
    padding: "0",
    border: "0",
    borderRadius: "3px",
    color: "inherit",
    backgroundColor: "transparent",
    cursor: "pointer",
  },
  ".cm-source-link-icon button:hover": {
    color: "var(--mem-link)",
  },
  ".cm-source-link-icon button:focus-visible": {
    outline: "2px solid var(--mem-accent-indigo)",
    outlineOffset: "1px",
  },
  // Inline-block, so the line holds only the card: a block inside a line
  // would add an empty line box above and below it.
  ".cm-source-link-card": {
    display: "inline-block",
    width: "100%",
    maxWidth: "520px",
    verticalAlign: "top",
    margin: "0.15rem 0",
  },
  ".cm-source-saving": {
    display: "inline-block",
    width: "11px",
    height: "11px",
    marginLeft: "0.35em",
    verticalAlign: "-0.05em",
    border: "1.5px solid var(--mem-text-tertiary)",
    borderTopColor: "transparent",
    borderRadius: "50%",
    animation: "cm-source-saving-turn 0.8s linear infinite",
  },
  "@keyframes cm-source-saving-turn": {
    to: { transform: "rotate(360deg)" },
  },
  "@media (prefers-reduced-motion: reduce)": {
    ".cm-source-saving": { animation: "none", borderTopColor: "var(--mem-text-tertiary)", opacity: "0.6" },
  },
});

/**
 * Pasting one link saves it as a source at once; its site icon shows it was
 * saved, and a small turning ring beside it while saving. A note under the
 * line shows only when just the link was kept or saving failed. Deleting the
 * link deletes that source.
 */
export function sourcePaste(options: SourcePasteOptions): Extension {
  return [
    promptOptions.of(options),
    promptState,
    freshLink,
    promptDecorations(),
    removeWithLink,
    theme,
    Prec.highest(keymap.of([{
      key: "Escape",
      run: (view) => {
        const prompt = view.state.field(promptState).prompt;
        if (!prompt || prompt.phase === "saving") return false;
        view.dispatch({ effects: setPrompt.of(null) });
        return true;
      },
    }])),
    // Ahead of Markdown's own paste, which wraps a selection in a plain link.
    Prec.high(EditorView.domEventHandlers({
      paste(event, view) {
        const data = event.clipboardData;
        if (!data || (data.files?.length ?? 0) > 0) return false;
        const url = pastedSourceUrl(data.getData("text/plain"));
        if (!url || options.blocked(view) || !options.enabled()) return false;
        const { offered } = view.state.field(promptState);
        const known = sourceLinkInfo(view.state.field(sourceLinkUrls, false), url);
        const { from, to } = view.state.selection.main;
        if (!pasteTargetsProse(view.state, from)) return false;
        // Pasted over a few words, the link takes them as its name.
        const selected = view.state.sliceDoc(from, to).trim();
        const label = selected && !selected.includes("\n") && selected.length <= 140 && !pastedSourceUrl(selected)
          && pasteTargetsProse(view.state, to) ? selected : undefined;
        if (known !== undefined) {
          // Saved before: it reads as that source here too, saved once.
          event.preventDefault();
          const markdown = sourceLinkMarkdown(label ?? known.title, url, showsAsCard(view.state, from, to, known.previewable));
          view.dispatch({
            changes: { from, to, insert: markdown },
            selection: { anchor: from + markdown.length },
            effects: setFreshLink.of({ from, to: from + markdown.length }),
            userEvent: "input.paste",
            scrollIntoView: true,
          });
          return true;
        }
        if (offered.has(url)) return false;
        event.preventDefault();
        const id = nextPromptId++;
        // Named by the writer, it is a link at once and keeps that name.
        const insert = label ? sourceLinkMarkdown(label, url) : url;
        const end = from + insert.length;
        view.dispatch({
          changes: { from, to, insert },
          selection: { anchor: end },
          effects: [
            setPrompt.of({ id, from, to: end, url, label, phase: "saving" }),
            ...(label ? [setFreshLink.of({ from, to: end })] : []),
          ],
          userEvent: "input.paste",
          scrollIntoView: true,
        });
        save(view, id);
        return true;
      },
    })),
  ];
}
