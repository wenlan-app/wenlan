// SPDX-License-Identifier: AGPL-3.0-only
import { syntaxTree } from "@codemirror/language";
import { Annotation, Facet, Prec, StateEffect, StateField, type EditorState, type Extension } from "@codemirror/state";
import { Decoration, EditorView, keymap, WidgetType } from "@codemirror/view";

/** Draws a link's site icon into `container`; returns its cleanup. */
export type SourceIconMount = (container: HTMLElement, url: string) => () => void;

export interface PastedSourceSaved {
  title: string;
  /** Only the link and its title were saved: the site needs a sign-in. */
  linkOnly: boolean;
  /** Take the save back: the link is no longer a source of the page. */
  undo(): Promise<void>;
}

export interface SourcePasteLabels {
  save: string;
  dismiss: string;
  saving: string;
  saved: string;
  savedLinkOnly: string;
  failed: string;
  retry: string;
  undo: string;
}

export interface SourcePasteOptions {
  blocked(view: EditorView): boolean;
  /** Whether a pasted link may be offered as a source right now. */
  enabled(): boolean;
  /** "auto" saves a pasted link at once, with an undo; "ask" offers a button first. */
  mode: "auto" | "ask";
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

type PromptPhase = "ask" | "saving" | "saved" | "linkOnly" | "failed";

interface Prompt {
  id: number;
  from: number;
  to: number;
  url: string;
  phase: PromptPhase;
}

interface PromptState {
  prompt: Prompt | null;
  /** Links already offered in this editing session; each is offered once. */
  offered: ReadonlySet<string>;
}

const setPrompt = StateEffect.define<Prompt | null>();
const ownChange = Annotation.define<true>();
let nextPromptId = 0;
/** What each finished save needs to be taken back, by prompt id. */
const savedLinks = new Map<number, { markdown: string; undo(): Promise<void> }>();

const promptState: StateField<PromptState> = StateField.define<PromptState>({
  create: () => ({ prompt: null, offered: new Set() }),
  update(value, transaction) {
    let { prompt, offered } = value;
    if (prompt && transaction.docChanged) {
      // Typing anything else is the answer "no": the link stays as it is.
      if (prompt.phase !== "saving" && !transaction.annotation(ownChange)) prompt = null;
      else if (prompt) prompt = { ...prompt, from: transaction.changes.mapPos(prompt.from, 1), to: transaction.changes.mapPos(prompt.to, -1) };
    }
    if (prompt?.phase === "ask" && transaction.selection && !transaction.effects.some((effect) => effect.is(setPrompt))) {
      const line = transaction.state.doc.lineAt(prompt.to);
      const head = transaction.state.selection.main.head;
      if (head < line.from || head > line.to) prompt = null;
    }
    for (const effect of transaction.effects) {
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

function undo(view: EditorView, id: number): void {
  const prompt = currentPrompt(view.state, id);
  const saved = savedLinks.get(id);
  savedLinks.delete(id);
  if (!prompt || !saved) return;
  // The link goes back to the plain address it was pasted as.
  const restore = view.state.sliceDoc(prompt.from, prompt.to) === saved.markdown && !view.state.readOnly;
  view.dispatch({
    changes: restore ? { from: prompt.from, to: prompt.to, insert: prompt.url } : undefined,
    effects: setPrompt.of(null),
    annotations: ownChange.of(true),
    userEvent: "input.paste",
  });
  void saved.undo().catch(() => undefined);
}

function save(view: EditorView, id: number): void {
  const options = view.state.facet(promptOptions);
  const prompt = currentPrompt(view.state, id);
  if (!options || !prompt || prompt.phase === "saving" || options.blocked(view)) return;
  update(view, id, "saving");
  void options.save(prompt.url).then(
    (saved) => {
      if (!view.dom.isConnected) return;
      const current = currentPrompt(view.state, id);
      if (!current) return;
      const phase: PromptPhase = saved.linkOnly ? "linkOnly" : "saved";
      let markdown = sourceLinkMarkdown(saved.title, current.url);
      // The link reads as its title once saved, unless the writer changed it meanwhile.
      const retitle = view.state.sliceDoc(current.from, current.to) === current.url && markdown !== current.url && !view.state.readOnly;
      if (!retitle) markdown = view.state.sliceDoc(current.from, current.to);
      savedLinks.set(id, { markdown, undo: saved.undo });
      if (retitle) {
        view.dispatch({
          changes: { from: current.from, to: current.to, insert: markdown },
          effects: setPrompt.of({ ...current, to: current.from + markdown.length, phase }),
          annotations: ownChange.of(true),
          userEvent: "input.paste",
        });
      } else {
        update(view, id, phase);
      }
      window.setTimeout(() => {
        if (view.dom.isConnected && currentPrompt(view.state, id)?.phase === phase) update(view, id, null);
        savedLinks.delete(id);
      }, 6000);
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
    return other.prompt.id === this.prompt.id && other.prompt.phase === this.prompt.phase && other.prompt.url === this.prompt.url;
  }

  toDOM(view: EditorView): HTMLElement {
    const labels = view.state.facet(promptOptions)?.labels();
    const row = document.createElement("div");
    row.className = "cm-source-paste";
    row.dataset.phase = this.prompt.phase;
    if (!labels) return row;
    const { id, phase, url } = this.prompt;
    const button = (text: string, className: string, onClick: () => void, icon = false): HTMLButtonElement => {
      const element = document.createElement("button");
      element.type = "button";
      element.className = className;
      if (icon) {
        const holder = document.createElement("span");
        holder.className = "cm-source-paste-icon";
        holder.setAttribute("aria-hidden", "true");
        mountIcon(view, holder, url);
        element.append(holder);
      }
      element.append(text);
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
    if (phase === "ask") {
      row.append(button(labels.save, "cm-source-paste-save", () => save(view, id), true));
      const dismiss = button("", "cm-source-paste-dismiss", () => update(view, id, null));
      dismiss.setAttribute("aria-label", labels.dismiss);
      dismiss.title = labels.dismiss;
      dismiss.textContent = "×";
      row.append(dismiss);
    } else if (phase === "saving") {
      row.append(status(labels.saving));
    } else if (phase === "failed") {
      row.append(status(labels.failed), button(labels.retry, "cm-source-paste-save", () => save(view, id)));
    } else {
      row.append(status(phase === "linkOnly" ? labels.savedLinkOnly : labels.saved));
      if (savedLinks.has(id)) row.append(button(labels.undo, "cm-source-paste-undo", () => undo(view, id)));
    }
    return row;
  }

  destroy(dom: HTMLElement): void {
    for (const holder of dom.querySelectorAll<HTMLElement>(".cm-source-paste-icon")) unmountIcon(holder);
  }

  ignoreEvent(): boolean {
    return true;
  }
}

/** Code and link destinations are not places to offer a source. */
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
    display: "inline-flex",
    alignItems: "center",
    gap: "6px",
    border: "0",
    borderRadius: "6px",
    font: "inherit",
    cursor: "pointer",
  },
  ".cm-source-paste-save": {
    padding: "3px 8px",
    color: "var(--mem-text)",
    backgroundColor: "var(--mem-hover)",
  },
  ".cm-source-paste-save:hover": {
    backgroundColor: "color-mix(in srgb, var(--mem-text) 10%, transparent)",
  },
  ".cm-source-paste-dismiss": {
    justifyContent: "center",
    width: "24px",
    height: "24px",
    padding: "0",
    fontSize: "16px",
    color: "var(--mem-text-tertiary)",
    backgroundColor: "transparent",
  },
  ".cm-source-paste-dismiss:hover": {
    color: "var(--mem-text)",
    backgroundColor: "var(--mem-hover)",
  },
  ".cm-source-paste button:focus-visible": {
    outline: "2px solid var(--mem-accent-indigo)",
    outlineOffset: "1px",
  },
  ".cm-source-paste-status": {
    padding: "3px 0",
  },
  ".cm-source-paste-undo": {
    marginLeft: "6px",
    padding: "3px 6px",
    color: "var(--mem-link)",
    backgroundColor: "transparent",
  },
  ".cm-source-paste-undo:hover": {
    backgroundColor: "var(--mem-hover)",
  },
  ".cm-source-paste[data-phase=saved] .cm-source-paste-status, .cm-source-paste[data-phase=linkOnly] .cm-source-paste-status": {
    color: "var(--mem-accent-sage)",
  },
  ".cm-source-paste-icon, .cm-source-link-icon": {
    display: "inline-flex",
    alignItems: "center",
    width: "14px",
    height: "14px",
  },
  ".cm-source-link-icon": {
    marginLeft: "0.2em",
    verticalAlign: "-0.12em",
    color: "var(--mem-text-tertiary)",
  },
});

/**
 * Pasting one link saves it as a source of the page, or offers to. The note
 * sits under the line. In "ask" mode, ignoring it (typing on, Escape, ×)
 * leaves a plain link; in "auto" mode the note offers an undo for a moment.
 */
export function sourcePaste(options: SourcePasteOptions): Extension {
  return [
    promptOptions.of(options),
    promptState,
    promptDecorations(),
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
          effects: setPrompt.of({ id, from, to: from + url.length, url, phase: "ask" }),
          userEvent: "input.paste",
          scrollIntoView: true,
        });
        if (options.mode === "auto") save(view, id);
        return true;
      },
    }),
  ];
}
