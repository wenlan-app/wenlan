// SPDX-License-Identifier: AGPL-3.0-only
import { ensureSyntaxTree, syntaxTree } from "@codemirror/language";
import { Prec, StateEffect, StateField, type Extension, type Range } from "@codemirror/state";
import { Decoration, type DecorationSet, EditorView, ViewPlugin, type ViewUpdate } from "@codemirror/view";
import { processCitations } from "../../../lib/pageCitations";
import type { MemoryItem, PageCitation } from "../../../lib/tauri";
import { referenceTargetFromHref, type ReferenceTarget } from "../links/referenceTypes";
import { getSourceKind } from "../sources/sourcePresentation";
import { setWritingCompositionActive } from "./writingPresentation";

export interface ParsedWikiLink {
  from: number;
  to: number;
  tokenFrom: number;
  tokenTo: number;
  targetLabel: string;
  displayText: string;
}

export interface EditorReferenceContext {
  citations?: PageCitation[];
  sourceMemories?: ReadonlyMap<string, MemoryItem>;
  sourcesLoading?: boolean;
  labels?: { page: string; memory: string; source: string; authored: string; unverified: string };
}
interface ResolvedReferenceRange extends ParsedWikiLink {
  target: ReferenceTarget;
  key: string;
  wiki: boolean;
}
export interface WikiLinkCallbacks {
  onActivate(pageId: string, anchor: HTMLAnchorElement): void;
  onPreview(pageId: string, anchor: HTMLAnchorElement, keyboard: boolean): void;
  onPreviewLeave(): void;
  onReferenceActivate?(target: ReferenceTarget, anchor: HTMLAnchorElement): void;
  onReferencePreview?(target: ReferenceTarget, anchor: HTMLAnchorElement, keyboard: boolean): void;
}

const wikiLinkTargetsEffect = StateEffect.define<ReadonlyMap<string, string>>();
const referenceContextEffect = StateEffect.define<EditorReferenceContext>();
export const setEditorReferenceContext = (context: EditorReferenceContext) => referenceContextEffect.of(context);
const refreshWikiLinkPresentation = StateEffect.define<null>();
const wikiPresentationRevision = StateField.define<number>({
  create: () => 0,
  update(value, transaction) {
    return transaction.effects.some((effect) => effect.is(refreshWikiLinkPresentation)) ? value + 1 : value;
  },
});
const wikiCompositionState = StateField.define<boolean>({
  create: () => false,
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setWritingCompositionActive)) value = effect.value;
    }
    return value;
  },
});
export function setResolvedWikiLinkTargets(targets: ReadonlyMap<string, string>) {
  return wikiLinkTargetsEffect.of(targets);
}

const excludedNodeNames = new Set([
  "CodeBlock", "FencedCode", "InlineCode", "HTMLBlock", "HTMLTag",
  "Comment", "CommentBlock", "ProcessingInstruction", "ProcessingInstructionBlock",
]);

function normalized(label: string): string {
  return label.trim().toLowerCase();
}

function excludedRanges(source: string, tree: ReturnType<typeof syntaxTree>): Array<{ from: number; to: number }> {
  const ranges: Array<{ from: number; to: number }> = [];
  let frontmatter = source.startsWith("---\n") || source.startsWith("---\r\n");
  if (frontmatter) {
    const delimiter = /\n(?:---|\.\.\.)[ \t]*(?:\r?\n|$)/.exec(source);
    ranges.push({ from: 0, to: delimiter ? delimiter.index + delimiter[0].length : source.length });
  }
  tree.iterate({ enter(node) {
    if (excludedNodeNames.has(node.name)) ranges.push({ from: node.from, to: node.to });
  } });
  return ranges;
}

export function parseResolvedWikiLinks(source: string, targets: ReadonlyMap<string, string>, baseOffset = 0): ParsedWikiLink[] {
  const matches: ParsedWikiLink[] = [];
  const pattern = /\[\[([^\]\n]+)\]\]/g;
  let match: RegExpExecArray | null;
  while ((match = pattern.exec(source))) {
    const from = match.index;
    let slashes = 0;
    for (let i = from - 1; i >= 0 && source[i] === "\\"; i--) slashes++;
    if (slashes % 2) continue;
    const inner = match[1];
    const pipe = inner.indexOf("|");
    const rawTarget = pipe >= 0 ? inner.slice(0, pipe) : inner;
    const hash = rawTarget.indexOf("#");
    const targetLabel = (hash >= 0 ? rawTarget.slice(0, hash) : rawTarget).trim();
    const alias = pipe >= 0 ? inner.slice(pipe + 1).trim() : "";
    const displayText = alias || targetLabel || rawTarget.trim() || inner.trim();
    const id = targets.get(normalized(targetLabel));
    if (!id || !targetLabel || !displayText) continue;
    const innerOffset = from + 2;
    const visibleFrom = alias
      ? innerOffset + pipe + 1 + inner.slice(pipe + 1).indexOf(alias)
      : innerOffset + rawTarget.indexOf(targetLabel);
    matches.push({
      from: baseOffset + visibleFrom,
      to: baseOffset + visibleFrom + displayText.length,
      tokenFrom: baseOffset + from,
      tokenTo: baseOffset + from + match[0].length,
      targetLabel,
      displayText,
    });
  }
  return matches;
}

function overlap(a: { from: number; to: number }, b: { from: number; to: number }): boolean {
  return a.from < b.to && b.from < a.to;
}

export function createWikiLinkEditing(
  targets: ReadonlyMap<string, string>,
  callbacks: WikiLinkCallbacks,
  context: EditorReferenceContext = {},
): Extension {
  const referenceContext = StateField.define<EditorReferenceContext>({
    create: () => context,
    update(value, transaction) {
      for (const effect of transaction.effects) if (effect.is(referenceContextEffect)) value = effect.value;
      return value;
    },
  });
  const links = StateField.define<ReadonlyMap<string, string>>({
    create: () => targets,
    update(value, transaction) {
      for (const effect of transaction.effects) if (effect.is(wikiLinkTargetsEffect)) value = effect.value;
      return value;
    },
  });
  const plugin = ViewPlugin.fromClass(class {
    decorations: DecorationSet;
    atomic: DecorationSet;
    references = new Map<string, ResolvedReferenceRange>();
    pointerGestureActive = false;
    pointerType = "";
    pendingTouchKey: string | null = null;
    pointerAnchor: HTMLAnchorElement | null = null;
    pointerFallbackTimer: number | null = null;
    keyboardAnchor: HTMLAnchorElement | null = null;
    private pointerListenersActive = false;
    constructor(readonly view: EditorView) { this.decorations = Decoration.none; this.atomic = Decoration.none; this.rebuild(); }
    update(update: ViewUpdate) {
      const invalidated = update.docChanged || update.state.field(links) !== update.startState.field(links) || update.state.field(referenceContext) !== update.startState.field(referenceContext);
      if (invalidated || update.selectionSet) {
        this.pendingTouchKey = null;
        this.keyboardAnchor = null;
      }
      if (this.pointerGestureActive && invalidated) this.endPointerGesture(false);
      if (this.pointerGestureActive) return;
      if (invalidated || update.viewportChanged || update.state.field(links) !== update.startState.field(links)
        || update.selectionSet || update.state.field(wikiPresentationRevision) !== update.startState.field(wikiPresentationRevision)
        || update.state.field(wikiCompositionState) !== update.startState.field(wikiCompositionState)
        || syntaxTree(update.state) !== syntaxTree(update.startState)) this.rebuild();
    }
    beginPointerGesture(anchor: HTMLAnchorElement, pointerType = ""): void {
      if (this.pointerGestureActive) this.endPointerGesture(false);
      this.pointerGestureActive = true;
      this.pointerType = pointerType;
      this.pendingTouchKey = pointerType === "touch" ? anchor.dataset.inlineReferenceKey ?? null : null;
      this.pointerAnchor = anchor;
      this.pointerListenersActive = true;
      document.addEventListener("pointerup", this.onDocumentPointerUp, true);
      document.addEventListener("pointercancel", this.onDocumentPointerCancel, true);
      document.addEventListener("focusin", this.onDocumentFocusIn, true);
      window.addEventListener("blur", this.onWindowBlur, true);
    }
    endPointerGesture(refresh = true): void {
      if (this.pointerFallbackTimer !== null) window.clearTimeout(this.pointerFallbackTimer);
      this.pointerFallbackTimer = null;
      if (this.pointerListenersActive) {
        document.removeEventListener("pointerup", this.onDocumentPointerUp, true);
        document.removeEventListener("pointercancel", this.onDocumentPointerCancel, true);
        document.removeEventListener("focusin", this.onDocumentFocusIn, true);
        window.removeEventListener("blur", this.onWindowBlur, true);
      }
      this.pointerListenersActive = false;
      const wasActive = this.pointerGestureActive;
      this.pointerGestureActive = false;
      this.pointerType = "";
      this.pointerAnchor = null;
      if (refresh && wasActive) this.view.dispatch({ effects: refreshWikiLinkPresentation.of(null) });
    }
    destroy(): void {
      this.keyboardAnchor = null;
      this.endPointerGesture(false);
    }
    private onDocumentPointerUp = (event: PointerEvent): void => {
      if (!this.pointerGestureActive) return;
      const anchor = anchorFrom(event);
      if (anchor && anchor === this.pointerAnchor) {
        // Keep the decorated anchor stable through the following click event.
        this.pointerFallbackTimer = window.setTimeout(() => this.endPointerGesture(), 0);
      } else {
        this.pendingTouchKey = null;
        this.endPointerGesture();
      }
    };
    private onDocumentPointerCancel = (): void => { this.pendingTouchKey = null; this.endPointerGesture(); };
    private onDocumentFocusIn = (event: FocusEvent): void => {
      if (event.target instanceof Node && !this.view.dom.contains(event.target)) this.endPointerGesture();
    };
    private onWindowBlur = (event: FocusEvent): void => {
      // Capture also observes descendant blur when WebKit focuses the tapped link.
      if (event.target !== window) return;
      this.pendingTouchKey = null;
      this.endPointerGesture();
    };
    rebuild(): void {
      const source = this.view.state.doc.toString();
      const state = this.view.state;
      const visibleTo = Math.max(0, ...this.view.visibleRanges.map((visible) => visible.to));
      // Other extensions may finish parsing before the state snapshot receives the tree.
      // Use the covering parser-context tree, or leave the source undecorated.
      const tree = ensureSyntaxTree(state, visibleTo, 20);
      if (!tree) {
        this.references = new Map();
        this.decorations = Decoration.none;
        this.atomic = Decoration.none;
        return;
      }
      const excluded = excludedRanges(source, tree);
      const currentContext = this.view.state.field(referenceContext);
      const resolvedLinks: ResolvedReferenceRange[] = parseResolvedWikiLinks(source, this.view.state.field(links))
        .map((range) => ({ ...range, target: { kind: "page", id: this.view.state.field(links).get(normalized(range.targetLabel))! }, key: `wiki:${range.tokenFrom}`, wiki: true }));
      // Use the Markdown parser, not title matching or guessed source IDs.
      tree.iterate({ enter(node) {
        if (node.name !== "Link") return;
        const marks = [];
        let url = null;
        for (let child = node.node.firstChild; child; child = child.nextSibling) {
          if (child.name === "LinkMark") marks.push(child);
          if (child.name === "URL") url = child;
        }
        if (!url || marks.length < 4) return;
        const href = source.slice(url.from, url.to);
        const target = referenceTargetFromHref(href);
        if (!target) return;
        const displayText = source.slice(marks[0].to, marks[1].from);
        if (!displayText) return;
        resolvedLinks.push({ from: marks[0].to, to: marks[1].from, tokenFrom: node.from, tokenTo: node.to,
          targetLabel: displayText, displayText, key: `link:${node.from}`, wiki: false,
          target });
      } });
      // Occurrence counting includes code, exactly as the backend does. Only a
      // complete, matching citation map can turn a marker into an active link.
      const citations = processCitations(source, currentContext.citations);
      let occurrence = 0;
      for (const match of source.matchAll(/\[(\d+)\]/g)) {
        occurrence++;
        const citation = citations.byOccurrence.get(occurrence);
        if (!citation || citation.source_kind === "authored") continue;
        const tokenFrom = match.index!;
        const tokenTo = tokenFrom + match[0].length;
        if (resolvedLinks.some((range) => overlap({ from: tokenFrom, to: tokenTo }, { from: range.tokenFrom, to: range.tokenTo }))) continue;
        let escapes = 0;
        for (let index = tokenFrom - 1; index >= 0 && source[index] === "\\"; index--) escapes++;
        if (escapes % 2) continue;
        resolvedLinks.push({ from: tokenFrom + 1, to: tokenTo - 1, tokenFrom, tokenTo,
          targetLabel: match[1], displayText: match[1], key: `citation:${occurrence}`, wiki: false,
          target: { kind: "citation", citation, sourceMemory: currentContext.sourceMemories?.get(citation.locator) ?? null,
            sourcesLoading: currentContext.sourcesLoading ?? false } });
      }
      const visibleLinks = resolvedLinks.filter((range) =>
        this.view.visibleRanges.some((visible) => overlap(range, visible))
        && !excluded.some((blocked) => overlap({ from: range.tokenFrom, to: range.tokenTo }, blocked)))
        .filter((range) => !/<\/?[A-Za-z][^>]*>/.test(this.view.state.doc.lineAt(range.from).text));
      this.references = new Map(visibleLinks.map((range) => [range.key, range]));
      const decorations: Array<Range<Decoration>> = [];
      const atomic: Array<Range<Decoration>> = [];
      const compositionActive = this.view.state.field(wikiCompositionState);
      const isEditing = this.view.contentDOM === this.view.dom.ownerDocument.activeElement;
      const activeRanges = this.view.state.selection.ranges;
      for (const range of visibleLinks) {
        const editingToken = isEditing && activeRanges.some((selection) => selection.empty
          ? selection.head >= range.tokenFrom && selection.head < range.tokenTo
          : selection.from < range.tokenTo && selection.to > range.tokenFrom);
        const target = range.target;
        const kind = target.kind === "citation" ? (target.citation.source_kind === "memory" ? "memory" : "source") : target.kind;
        const unverified = target.kind === "citation" && target.citation.status === "unverified";
        const typeLabel = currentContext.labels?.[kind] ?? kind;
        const href = target.kind === "page" ? `#concept:${target.id}` : target.kind === "memory" ? `#memory:${target.id}` : `#citation:${target.citation.occurrence}`;
        decorations.push(Decoration.mark({
          tagName: "a",
          class: `cm-wiki-page-link reference-link reference-link--${kind}${target.kind === "citation" ? " cm-citation-link" : ""}${unverified ? " reference-link--unverified" : ""}`,
          attributes: {
            href, role: "link", tabindex: "0",
            "aria-label": unverified ? `${range.displayText}, ${typeLabel}, ${currentContext.labels?.unverified ?? "Unverified"}` : range.displayText,
            "title": unverified ? `${typeLabel} · ${currentContext.labels?.unverified ?? "Unverified"}` : typeLabel,
            "data-inline-reference-key": range.key,
            "data-reference-resource-icon": target.kind === "citation" && target.citation.source_kind !== "memory"
              ? getSourceKind({ source: target.citation.source_kind === "external_url" ? "webpage" : "file", source_id: target.citation.locator, title: "" })
              : kind,
            ...(target.kind === "page" ? { "data-wiki-page-id": target.id } : {}),
            "data-wiki-target-label": range.targetLabel,
          },
        }).range(range.from, range.to));
        if (compositionActive || editingToken) continue;
        if (range.tokenFrom < range.from) {
          const concealed = Decoration.replace({}).range(range.tokenFrom, range.from);
          decorations.push(concealed);
          atomic.push(concealed);
        }
        if (range.to < range.tokenTo) {
          const concealed = Decoration.replace({}).range(range.to, range.tokenTo);
          decorations.push(concealed);
          atomic.push(concealed);
        }
      }
      this.decorations = Decoration.set(decorations, true);
      this.atomic = Decoration.set(atomic, true);
    }
  }, { decorations: (value) => value.decorations });
  const anchorFrom = (event: Event): HTMLAnchorElement | null => {
    const target = event.target;
    if (!(target instanceof Element)) return null;
    const anchor = target.closest<HTMLAnchorElement>("a[data-inline-reference-key]");
    return anchor;
  };
  const activate = (view: EditorView, anchor: HTMLAnchorElement) => {
    const reference = view.plugin(plugin)?.references.get(anchor.dataset.inlineReferenceKey!);
    if (!reference) return;
    if (reference.wiki && reference.target.kind === "page") callbacks.onActivate(reference.target.id, anchor);
    else callbacks.onReferenceActivate?.(reference.target, anchor);
  };
  const preview = (view: EditorView, anchor: HTMLAnchorElement, keyboard: boolean) => {
    const reference = view.plugin(plugin)?.references.get(anchor.dataset.inlineReferenceKey!);
    if (!reference) return;
    if (reference.wiki && reference.target.kind === "page") callbacks.onPreview(reference.target.id, anchor, keyboard);
    else callbacks.onReferencePreview?.(reference.target, anchor, keyboard);
  };
  return [
    links,
    referenceContext,
    wikiPresentationRevision,
    wikiCompositionState,
    plugin,
    EditorView.atomicRanges.of((view) => view.plugin(plugin)?.atomic ?? Decoration.none),
    Prec.highest(EditorView.domEventHandlers({
      focus(_event, view) {
        view.dispatch({ effects: refreshWikiLinkPresentation.of(null) });
        return false;
      },
      blur(_event, view) {
        view.dispatch({ effects: refreshWikiLinkPresentation.of(null) });
        return false;
      },
      click(event, view) {
        const anchor = anchorFrom(event);
        const viewPlugin = view.plugin(plugin);
        const pointerGesture = viewPlugin?.pointerGestureActive ?? false;
        const pointerAnchor = viewPlugin?.pointerAnchor ?? null;
        if (!anchor || event.defaultPrevented) {
          if (viewPlugin) viewPlugin.keyboardAnchor = null;
          if (pointerGesture) viewPlugin?.endPointerGesture();
          return false;
        }
        if (viewPlugin) viewPlugin.keyboardAnchor = null;
        const pointer = event as MouseEvent;
        if (pointer.button !== 0) return false;
        event.preventDefault();
        // WebKit emits a delayed compatibility click with pointerType="mouse"
        // after a touch pointerup; retain identity beyond the gesture timer.
        const touched = viewPlugin?.pendingTouchKey === anchor.dataset.inlineReferenceKey;
        if (viewPlugin) viewPlugin.pendingTouchKey = null;
        try {
          if (pointerGesture && anchor !== pointerAnchor) return true;
          if (pointer.altKey || pointer.ctrlKey || pointer.metaKey || pointer.shiftKey || !view.state.selection.main.empty || view.compositionStarted) return true;
          if (touched || viewPlugin?.pointerType === "touch" || ("pointerType" in pointer && (pointer as PointerEvent).pointerType === "touch")) {
            preview(view, anchor, false);
            return true;
          }
          activate(view, anchor);
          return true;
        } finally {
          if (pointerGesture) viewPlugin?.endPointerGesture();
        }
      },
      pointerdown(event, view) {
        const anchor = anchorFrom(event);
        const pointer = event as PointerEvent;
        if (!anchor) {
          const value = view.plugin(plugin);
          if (value) {
            value.pendingTouchKey = null;
            value.keyboardAnchor = null;
          }
          return false;
        }
        const linkPlugin = view.plugin(plugin);
        if (linkPlugin) linkPlugin.keyboardAnchor = null;
        if (pointer.button !== 0 || pointer.isPrimary === false || pointer.defaultPrevented) return false;
        view.plugin(plugin)?.beginPointerGesture(anchor, pointer.pointerType);
        return false;
      },
      keydown(event, view) {
        const viewPlugin = view.plugin(plugin);
        const eventAnchor = anchorFrom(event);
        const retargetedAnchor = !eventAnchor && event.target === view.contentDOM && view.root.activeElement === view.contentDOM
          && viewPlugin?.keyboardAnchor?.isConnected && view.contentDOM.contains(viewPlugin.keyboardAnchor)
          && viewPlugin.references.has(viewPlugin.keyboardAnchor.dataset.inlineReferenceKey ?? "")
          ? viewPlugin.keyboardAnchor
          : null;
        const candidate = eventAnchor ?? retargetedAnchor;
        const referenceKey = candidate?.dataset.inlineReferenceKey;
        const keyboardAnchor = candidate && candidate.isConnected && view.contentDOM.contains(candidate)
          && referenceKey && viewPlugin?.references.has(referenceKey)
          ? candidate
          : null;
        if (event.key === "Enter") {
          if (!keyboardAnchor || event.altKey || event.ctrlKey || event.metaKey || event.shiftKey || view.compositionStarted || event.isComposing) {
            if (viewPlugin) viewPlugin.keyboardAnchor = null;
            return false;
          }
          if (viewPlugin) viewPlugin.keyboardAnchor = null;
          event.preventDefault();
          event.stopPropagation();
          activate(view, keyboardAnchor);
          return true;
        }
        if (event.key === "Tab") {
          if (viewPlugin) viewPlugin.keyboardAnchor = null;
          return false;
        }
        if (viewPlugin) viewPlugin.keyboardAnchor = null;
        return false;
      },
      focusin(event, view) {
        const viewPlugin = view.plugin(plugin);
        const anchor = anchorFrom(event);
        if (anchor) {
          const isNewKeyboardAnchor = viewPlugin?.keyboardAnchor !== anchor;
          if (viewPlugin) viewPlugin.keyboardAnchor = anchor;
          if (isNewKeyboardAnchor) preview(view, anchor, true);
          return false;
        }
        const remembered = viewPlugin?.keyboardAnchor;
        if (event.target === view.contentDOM && remembered?.isConnected
          && view.contentDOM.contains(remembered)
          && (!event.relatedTarget || event.relatedTarget === remembered)) {
          // CodeMirror may reclaim focus from its decorated child before a key arrives.
          remembered.focus({ preventScroll: true });
          return false;
        }
        if (viewPlugin) viewPlugin.keyboardAnchor = null;
        return false;
      },
      pointerover(event, view) {
        const anchor = anchorFrom(event);
        if (!anchor) return false;
        const viewPlugin = view.plugin(plugin);
        if (viewPlugin) viewPlugin.keyboardAnchor = null;
        preview(view, anchor, false);
        return false;
      },
      pointerout(event, view) {
        const anchor = anchorFrom(event);
        if (!anchor) return false;
        const related = event.relatedTarget;
        if (related instanceof Node && anchor.contains(related)) return false;
        const viewPlugin = view.plugin(plugin);
        if (viewPlugin) viewPlugin.keyboardAnchor = null;
        callbacks.onPreviewLeave();
        return false;
      },
      focusout(event, view) {
        const anchor = anchorFrom(event);
        if (!anchor) return false;
        const related = event.relatedTarget;
        if (related === view.contentDOM) return false;
        if (related instanceof Node && (anchor.contains(related) || (related instanceof Element && related.closest("[data-page-link-preview]")))) return false;
        const viewPlugin = view.plugin(plugin);
        if (viewPlugin) viewPlugin.keyboardAnchor = null;
        callbacks.onPreviewLeave();
        return false;
      },
    })),
  ];
}
