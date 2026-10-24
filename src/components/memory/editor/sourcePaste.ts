// SPDX-License-Identifier: AGPL-3.0-only
import { syntaxTree } from "@codemirror/language";
import { Annotation, Facet, Prec, StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap, ViewPlugin, WidgetType } from "@codemirror/view";

/** Draws a link's site icon into `container`; returns its cleanup. */
export type SourceIconMount = (container: HTMLElement, url: string) => () => void;

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
  save(url: string): Promise<PastedSourceSaved>;
}

/** The links already saved as this page's sources; they show their site icon. */
export const setSourceLinkUrls = StateEffect.define<ReadonlySet<string>>();
export const sourceLinkUrls = StateField.define<ReadonlySet<string>>({
  create: () => new Set(),
  update(value, transaction) {
    for (const effect of transaction.effects) if (effect.is(setSourceLinkUrls)) value = effect.value;
    return value;
  },
});

export const sourceIconMount = Facet.define<SourceIconMount, SourceIconMount | null>({
  combine: (values) => values[0] ?? null,
});

const iconCleanups = new WeakMap<HTMLElement, () => void>();

function mountIcon(view: EditorView, container: HTMLElement, url: string): void {
  const mount = view.state.facet(sourceIconMount);
  if (mount) iconCleanups.set(container, mount(container, url));
}

function unmountIcon(container: HTMLElement): void {
  iconCleanups.get(container)?.();
  iconCleanups.delete(container);
}

/** The site icon after a link saved as a source, where other links show an arrow. */
export class SourceLinkIconWidget extends WidgetType {
  constructor(readonly url: string) {
    super();
  }

  eq(other: SourceLinkIconWidget): boolean {
    return other.url === this.url;
  }

  toDOM(view: EditorView): HTMLElement {
    const icon = document.createElement("span");
    icon.className = "cm-source-link-icon";
    icon.setAttribute("aria-hidden", "true");
    mountIcon(view, icon, this.url);
    return icon;
  }

  destroy(dom: HTMLElement): void {
    unmountIcon(dom);
  }
}

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
export function sourceLinkMarkdown(title: string, url: string): string {
  const text = title.replace(/\s+/g, " ").trim().slice(0, 140).replace(/[\\[\]]/g, "\\$&");
  return text ? `[${text}](${url})` : url;
}

type PromptPhase = "saving" | "linkOnly" | "failed";

interface Prompt {
  id: number;
  from: number;
  to: number;
  url: string;
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
    const at = state.doc.lineAt(prompt.to).to;
    return Decoration.set([
      Decoration.widget({ widget: new PromptWidget(prompt), block: true, side: 1 }).range(at),
    ]);
  });
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
  void options.save(prompt.url).then(
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
      const markdown = sourceLinkMarkdown(saved.title, current.url);
      // The link reads as its title once saved, unless the writer changed it meanwhile.
      if (view.state.sliceDoc(current.from, current.to) === current.url && markdown !== current.url && !view.state.readOnly) {
        view.dispatch({
          changes: { from: current.from, to: current.to, insert: markdown },
          effects: setPrompt.of(next && { ...next, to: current.from + markdown.length }),
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
    const kept = new Set([...text.matchAll(/https?:\/\/[^\s<>()[\]]+/g)].map((match) => linkKey(match[0])));
    for (const [url, remove] of [...created]) {
      // Trimming tracking parameters or a trailing slash still links the same page.
      if (text.includes(url) || kept.has(linkKey(url))) continue;
      created.delete(url);
      void remove().catch(() => undefined);
      if (live) this.view.dispatch({ effects: forgetLink.of(url) });
    }
  }
});

/** A link's site and path, so an edited query, fragment or trailing slash is the same page. */
function linkKey(link: string): string {
  try {
    const url = new URL(link);
    return `${url.hostname.replace(/^www\./, "")}${url.pathname.replace(/\/+$/, "")}`;
  } catch {
    return link;
  }
}

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
});

/**
 * Pasting one link saves it as a source at once; its site icon shows it was
 * saved. A note under the line shows only while saving, when the site needs
 * a sign-in, or when saving failed. Deleting the link deletes that source.
 */
export function sourcePaste(options: SourcePasteOptions): Extension {
  return [
    promptOptions.of(options),
    promptState,
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
    EditorView.domEventHandlers({
      paste(event, view) {
        const data = event.clipboardData;
        if (!data || (data.files?.length ?? 0) > 0) return false;
        const url = pastedSourceUrl(data.getData("text/plain"));
        if (!url || options.blocked(view) || !options.enabled()) return false;
        const { offered } = view.state.field(promptState);
        const known = view.state.field(sourceLinkUrls, false);
        const { from, to } = view.state.selection.main;
        if (offered.has(url) || known?.has(url) || !pasteTargetsProse(view.state, from)) return false;
        event.preventDefault();
        const id = nextPromptId++;
        view.dispatch({
          changes: { from, to, insert: url },
          selection: { anchor: from + url.length },
          effects: setPrompt.of({ id, from, to: from + url.length, url, phase: "saving" }),
          userEvent: "input.paste",
          scrollIntoView: true,
        });
        save(view, id);
        return true;
      },
    }),
  ];
}
