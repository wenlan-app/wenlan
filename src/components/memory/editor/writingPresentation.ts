// SPDX-License-Identifier: AGPL-3.0-only
import {
  ensureSyntaxTree,
  HighlightStyle,
  syntaxHighlighting,
  syntaxTree,
  syntaxTreeAvailable,
} from "@codemirror/language";
import {
  StateEffect,
  StateField,
  type Extension,
  type Range,
} from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  keymap,
  ViewPlugin,
  type ViewUpdate,
  WidgetType,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { hiddenUrlPrefixLength } from "../../../lib/displayUrl";
import { SourceLinkCardWidget, SourceLinkIconWidget, sourceLinkInfo, sourceLinkIsFresh, sourceLinkPlaceAt, sourceLinkUrls } from "./sourcePaste";
import {
  attachmentKind,
  attachmentName,
  attachmentUrl,
  primeAttachmentsDir,
  subscribeAttachmentsDir,
  videoSource,
  type AttachmentKind,
} from "../../../lib/pageAttachments";

export const setWritingCompositionActive = StateEffect.define<boolean>();
const refreshWritingPresentation = StateEffect.define<null>();

const writingCompositionActive = StateField.define<boolean>({
  create: () => false,
  update(value, transaction) {
    for (const effect of transaction.effects) {
      if (effect.is(setWritingCompositionActive)) value = effect.value;
    }
    return value;
  },
});



interface WritingRanges {
  decorations: DecorationSet;
  atomic: DecorationSet;
}

type TreeNodeLike = Readonly<{
  name: string;
  from: number;
  to: number;
  firstChild: TreeNodeLike | null;
  nextSibling: TreeNodeLike | null;
  parent: TreeNodeLike | null;
}>;

class ListMarkerWidget extends WidgetType {
  constructor(
    readonly kind: "bullet" | "ordered",
    readonly label: string,
  ) {
    super();
  }

  eq(other: ListMarkerWidget): boolean {
    return other.kind === this.kind && other.label === this.label;
  }

  toDOM(): HTMLElement {
    const marker = document.createElement("span");
    marker.className = "cm-writing-list-marker";
    marker.dataset.writingListMarker = this.kind;
    marker.setAttribute("aria-hidden", "true");
    marker.textContent = this.label;
    return marker;
  }
}

class TaskCheckboxWidget extends WidgetType {
  constructor(
    readonly from: number,
    readonly to: number,
    readonly checked: boolean,
    readonly label: string,
  ) {
    super();
  }

  eq(other: TaskCheckboxWidget): boolean {
    return (
      other.from === this.from &&
      other.to === this.to &&
      other.checked === this.checked &&
      other.label === this.label
    );
  }

  toDOM(view: EditorView): HTMLElement {
    const checkbox = document.createElement("input");
    checkbox.className = "cm-writing-task-checkbox";
    checkbox.type = "checkbox";
    checkbox.checked = this.checked;
    checkbox.dataset.writingTaskFrom = String(this.from);
    checkbox.setAttribute(
      "aria-label",
      this.label.trim() || (this.checked ? "[x]" : "[ ]"),
    );
    checkbox.addEventListener("change", () => {
      const current = view.state.sliceDoc(this.from, this.to);
      if (view.state.readOnly || !/^\[[ xX]\]$/.test(current)) {
        checkbox.checked = this.checked;
        return;
      }
      view.dispatch({
        changes: {
          from: this.from,
          to: this.to,
          insert: checkbox.checked ? "[x]" : "[ ]",
        },
        userEvent: "input",
      });
      view.dom
        .querySelector<HTMLInputElement>(
          `.cm-writing-task-checkbox[data-writing-task-from="${this.from}"]`,
        )
        ?.focus({ preventScroll: true });
    });
    return checkbox;
  }
}

class ImageWidget extends WidgetType {
  constructor(
    readonly source: string,
    readonly alt: string,
    readonly kind: AttachmentKind = "image",
    /** Shown above its own Markdown while the caret is in it. */
    readonly open = false,
  ) {
    super();
  }

  eq(other: ImageWidget): boolean {
    return other.source === this.source && other.alt === this.alt && other.kind === this.kind
      && other.open === this.open;
  }

  // A click on an image opens its source; a video keeps its own controls.
  ignoreEvent(event: Event): boolean {
    return this.kind === "video" || event.type !== "mousedown";
  }

  toDOM(): HTMLElement {
    if (this.kind === "video") {
      const video = document.createElement("video");
      video.className = `cm-writing-image cm-writing-video${this.open ? " cm-writing-image-open" : ""}`;
      video.src = videoSource(this.source);
      video.controls = true;
      video.preload = "metadata";
      if (this.alt) video.setAttribute("aria-label", this.alt);
      return video;
    }
    const image = document.createElement("img");
    image.className = `cm-writing-image${this.open ? " cm-writing-image-open" : ""}`;
    image.src = this.source;
    image.alt = this.alt;
    image.setAttribute("loading", "lazy");
    image.setAttribute("decoding", "async");
    image.setAttribute("referrerpolicy", "no-referrer");
    image.draggable = false;
    return image;
  }
}

class CalloutBadgeWidget extends WidgetType {
  constructor(
    readonly type: string,
    readonly label: string,
  ) {
    super();
  }

  eq(other: CalloutBadgeWidget): boolean {
    return other.type === this.type && other.label === this.label;
  }

  toDOM(): HTMLElement {
    const badge = document.createElement("span");
    badge.className = "cm-writing-callout-badge";
    badge.dataset.calloutType = this.type;
    badge.textContent = this.label;
    return badge;
  }
}

class HorizontalRuleWidget extends WidgetType {
  eq(other: HorizontalRuleWidget): boolean {
    return other instanceof HorizontalRuleWidget;
  }

  toDOM(): HTMLElement {
    const rule = document.createElement("span");
    rule.className = "cm-writing-horizontal-rule";
    rule.setAttribute("aria-hidden", "true");
    return rule;
  }
}

class WritingPresentationValue {
  decorations: DecorationSet;
  atomic: DecorationSet;
  private readonly stopAttachmentWatch: () => void;

  constructor(view: EditorView) {
    ({ decorations: this.decorations, atomic: this.atomic } = buildWritingRanges(view));
    // Page images from `_attachments/` show once the folder is known.
    this.stopAttachmentWatch = subscribeAttachmentsDir(() => {
      view.dispatch({ effects: refreshWritingPresentation.of(null) });
    });
  }

  destroy(): void {
    this.stopAttachmentWatch();
  }

  update(update: ViewUpdate): void {
    // Language parsing can advance in a transaction that does not change the
    // document or selection, so every view update is a valid rebuild signal.
    // The builder itself is bounded to CodeMirror's visible ranges.
    ({ decorations: this.decorations, atomic: this.atomic } = buildWritingRanges(update.view));
  }
}

const writingPresentationPlugin = ViewPlugin.fromClass(WritingPresentationValue, {
  decorations: (value) => value.decorations,
});

const writingHighlightStyle = HighlightStyle.define([
  { tag: tags.heading, class: "cm-writing-syntax-heading" },
  {
    tag: [tags.comment, tags.meta, tags.atom, tags.keyword],
    class: "cm-writing-syntax-muted",
  },
  { tag: tags.emphasis, class: "cm-writing-syntax-emphasis" },
]);

const writingTheme = EditorView.theme({
  "&[data-editor-presentation-mode=writing] .cm-scroller": {
    fontFamily: "var(--mem-font-body)",
  },
  "&[data-editor-presentation-mode=writing] .cm-content": {
    fontSize: "var(--reading-font-size, 15px)",
    lineHeight: "var(--reading-line-height, 1.75)",
  },
  ".cm-line": {
    paddingLeft: "0",
    paddingRight: "0",
  },
  ".cm-writing-heading": {
    color: "var(--mem-text)",
    fontFamily: "var(--mem-font-heading)",
    fontWeight: "500",
    lineHeight: "1.4",
  },
  ".cm-writing-heading-1": {
    fontSize: "20px",
  },
  ".cm-writing-heading-2": {
    fontSize: "18px",
  },
  ".cm-writing-heading-3, .cm-writing-heading-4, .cm-writing-heading-5, .cm-writing-heading-6": {
    fontSize: "15.5px",
  },
  "&[data-editor-presentation-mode=writing] .cm-writing-syntax-heading": {
    color: "var(--mem-text)",
    fontFamily: "inherit",
    fontWeight: "inherit",
  },
  "&[data-editor-presentation-mode=writing] .cm-writing-syntax-muted": {
    color: "var(--mem-text-tertiary)",
    fontWeight: "400",
  },
  "&[data-editor-presentation-mode=writing] .cm-writing-syntax-emphasis": {
    color: "var(--mem-text)",
  },
  ".cm-writing-strong": {
    fontWeight: "700",
  },
  ".cm-writing-emphasis": {
    fontStyle: "italic",
  },
  ".cm-writing-inline-code, .cm-writing-fenced-code": {
    fontFamily: "var(--mem-font-mono)",
  },
  ".cm-writing-inline-code": {
    padding: "0.08em 0.3em",
    borderRadius: "3px",
    backgroundColor: "var(--mem-hover)",
  },
  ".cm-writing-fenced-code": {
    boxSizing: "border-box",
    paddingLeft: "0.75rem",
    paddingRight: "0.75rem",
    backgroundColor: "var(--mem-hover)",
  },
  ".cm-writing-fenced-code-first": {
    paddingTop: "0.28rem",
    borderTopLeftRadius: "var(--mem-radius-sm)",
    borderTopRightRadius: "var(--mem-radius-sm)",
  },
  ".cm-writing-fenced-code-last": {
    paddingBottom: "0.28rem",
    borderBottomLeftRadius: "var(--mem-radius-sm)",
    borderBottomRightRadius: "var(--mem-radius-sm)",
  },
  ".cm-writing-strikethrough": {
    textDecoration: "line-through",
    textDecorationThickness: "1px",
  },
  ".cm-writing-link": {
    color: "var(--mem-link)",
    textDecoration: "underline",
    textDecorationColor: "color-mix(in srgb, var(--mem-link) 45%, transparent)",
    textUnderlineOffset: "0.16em",
  },
  ".cm-writing-link-external::after": {
    content: '"\\2197"',
    display: "inline-block",
    marginLeft: "0.15em",
    fontSize: "0.8em",
    lineHeight: "1",
    textDecoration: "none",
  },
  ".cm-writing-image": {
    boxSizing: "border-box",
    display: "inline-block",
    maxWidth: "100%",
    maxHeight: "20rem",
    border: "1px solid var(--mem-border)",
    borderRadius: "var(--mem-radius-sm)",
    objectFit: "contain",
    verticalAlign: "middle",
  },
  ".cm-writing-image-open": {
    display: "block",
    marginBottom: "0.25rem",
  },
  ".cm-writing-media-source": {
    color: "var(--mem-text-tertiary)",
    fontSize: "0.85em",
  },
  // Quiet the link and URL highlighting inside the source too.
  ".cm-writing-media-source *": {
    color: "inherit",
  },
  ".cm-writing-list-marker": {
    boxSizing: "border-box",
    display: "inline-block",
    minWidth: "1.35em",
    paddingRight: "0.35em",
    color: "var(--mem-text-secondary)",
    fontVariantNumeric: "tabular-nums",
    textAlign: "right",
    userSelect: "none",
  },
  '.cm-writing-list-marker[data-writing-list-marker="bullet"]': {
    textAlign: "center",
  },
  ".cm-writing-task-checkbox": {
    width: "0.95rem",
    height: "0.95rem",
    margin: "0 0.48rem 0 0",
    verticalAlign: "-0.12rem",
    accentColor: "var(--mem-accent-indigo)",
    cursor: "pointer",
  },
  ".cm-writing-task-checkbox:focus": {
    outline: "2px solid var(--mem-accent-page)",
    outlineOffset: "2px",
  },
  ".cm-writing-blockquote": {
    boxSizing: "border-box",
    borderLeft: "2px solid var(--mem-accent-indigo-border)",
    paddingLeft: "0.8rem",
    color: "var(--mem-text-secondary)",
  },
  ".cm-writing-callout": {
    borderLeftColor: "var(--mem-accent-indigo)",
    paddingRight: "0.75rem",
    backgroundColor:
      "color-mix(in srgb, var(--mem-indigo-bg) 55%, transparent)",
  },
  ".cm-writing-callout-badge": {
    boxSizing: "border-box",
    display: "inline-flex",
    alignItems: "center",
    minHeight: "1.25rem",
    marginRight: "0.45rem",
    padding: "0.05rem 0.42rem",
    border: "1px solid var(--mem-accent-indigo-border)",
    borderRadius: "999px",
    color: "var(--mem-accent-indigo)",
    backgroundColor: "var(--mem-detail-surface)",
    fontFamily: "var(--mem-font-body)",
    fontSize: "0.72em",
    fontWeight: "600",
    letterSpacing: "0.02em",
    lineHeight: "1",
    verticalAlign: "0.08em",
  },
  ".cm-writing-horizontal-rule": {
    boxSizing: "border-box",
    display: "inline-block",
    width: "100%",
    height: "1px",
    backgroundColor: "var(--mem-border)",
    verticalAlign: "middle",
  },
});

/** Links that leave the app for the browser; wiki references use "#" hrefs. */
function isBrowserUrl(url: string): boolean {
  return /^https?:\/\//i.test(url.trim());
}

export function writingPresentation(): Extension {
  return [
    writingCompositionActive,
    // Let CodeMirror synchronize the DOM selection before revealing its syntax.
    EditorView.focusChangeEffect.of(() =>
      refreshWritingPresentation.of(null),
    ),
    EditorView.editorAttributes.of({
      "data-editor-presentation-mode": "writing",
    }),
    writingPresentationPlugin,
    EditorView.domEventHandlers({ mousedown: revealClickedImage }),
    keymap.of([{ key: "Backspace", run: deleteMediaLineAbove }]),
    EditorView.atomicRanges.of(
      (view) => view.plugin(writingPresentationPlugin)?.atomic ?? Decoration.none,
    ),
    syntaxHighlighting(writingHighlightStyle),
    writingTheme,
  ];
}

function buildWritingRanges(view: EditorView): WritingRanges {
  const decorations: Array<Range<Decoration>> = [];
  const atomic: Array<Range<Decoration>> = [];
  const concealKeys = new Set<string>();
  const lineKeys = new Set<string>();
  const markKeys = new Set<string>();
  const state = view.state;
  const compositionActive = state.field(writingCompositionActive, false) ?? false;
  const sourceLinks = state.field(sourceLinkUrls, false);
  const contentFocused =
    view.contentDOM === view.contentDOM.ownerDocument.activeElement;
  const frontmatterEnd = findFrontmatterEnd(state);

  const addConcealment = (
    from: number,
    to: number,
    widget?: WidgetType,
  ): void => {
    if (compositionActive || from >= to) return;
    const key = `${from}:${to}`;
    if (concealKeys.has(key)) return;
    concealKeys.add(key);
    const range = Decoration.replace({ writingConceal: true, widget }).range(from, to);
    decorations.push(range);
    atomic.push(range);
  };

  const addLineClass = (at: number, className: string): void => {
    const key = `${at}:${className}`;
    if (lineKeys.has(key)) return;
    lineKeys.add(key);
    decorations.push(Decoration.line({ class: className }).range(at));
  };

  const addMarkClass = (from: number, to: number, className: string): void => {
    if (from >= to) return;
    const key = `${from}:${to}:${className}`;
    if (markKeys.has(key)) return;
    markKeys.add(key);
    decorations.push(Decoration.mark({ class: className }).range(from, to));
  };

  for (const visible of view.visibleRanges) {
    const tree = syntaxTreeAvailable(state, visible.to)
      ? syntaxTree(state)
      : ensureSyntaxTree(state, visible.to, 20);
    if (!tree) continue;
    tree.iterate({
      from: visible.from,
      to: visible.to,
      enter(node) {
        const parent = node.node.parent;

        if (/^ATXHeading[1-6]$/.test(node.name)) {
          const level = node.name.slice("ATXHeading".length);
          addLineClass(
            state.doc.lineAt(node.from).from,
            `cm-writing-heading cm-writing-heading-${level}`,
          );
          return;
        }

        if (node.name === "StrongEmphasis") {
          addMarkClass(node.from, node.to, "cm-writing-strong");
          return;
        }
        if (node.name === "Emphasis") {
          addMarkClass(node.from, node.to, "cm-writing-emphasis");
          return;
        }
        if (node.name === "InlineCode") {
          addMarkClass(node.from, node.to, "cm-writing-inline-code");
          return;
        }
        if (node.name === "Strikethrough") {
          addMarkClass(node.from, node.to, "cm-writing-strikethrough");
          return;
        }
        if (node.name === "Image") {
          const marks = directChildrenNamed(node.node, "LinkMark");
          const url = directChildrenNamed(node.node, "URL")[0];
          const opening = marks[0];
          const closingLabel = marks[1];
          if (!url || !opening || !closingLabel || marks.length < 4) return;
          const target = state.sliceDoc(url.from, url.to);
          const source = mediaSource(target);
          if (!source) return;
          const alt = state.sliceDoc(opening.to, closingLabel.from);
          const kind = attachmentKind(attachmentName(target) ?? "") ?? "image";
          if (!mediaIsRevealed(state, node.from, node.to, contentFocused)) {
            addConcealment(node.from, node.to, new ImageWidget(source, alt, kind));
            return;
          }
          if (compositionActive) return;
          // The image stays in view, its Markdown a quiet line beneath it.
          decorations.push(
            Decoration.widget({ widget: new ImageWidget(source, alt, kind, true), side: -1 }).range(node.from),
          );
          addMarkClass(node.from, node.to, "cm-writing-media-source");
          return;
        }
        if (node.name === "Link") {
          const marks = directChildrenNamed(node.node, "LinkMark");
          const url = directChildrenNamed(node.node, "URL")[0];
          const opening = marks[0];
          const closingLabel = marks[1];
          const closing = marks[marks.length - 1];
          if (url && /^#(?:concept|memory):[^\s#]+$/.test(state.sliceDoc(url.from, url.to))) return;
          if (url && marks.length >= 4 && opening && closingLabel && closing) {
            addMarkClass(opening.to, closingLabel.from, "cm-writing-link");
            // A link that just took its title stays a link beside the caret.
            if (!constructIsRevealed(state, node.from, node.to, contentFocused) || sourceLinkIsFresh(state, node.from, node.to)) {
              const target = state.sliceDoc(url.from, url.to);
              const source = sourceLinkInfo(sourceLinks, target);
              const place = source ? sourceLinkPlaceAt(state, node.from, node.to) : null;
              const canPreview = Boolean(source?.previewable && place?.alone);
              // A link alone on its line, marked so, reads as its source's card.
              if (source && canPreview && place?.preview) {
                addConcealment(node.from, node.to, new SourceLinkCardWidget(target, state.sliceDoc(opening.to, closingLabel.from), node.from));
                return;
              }
              addConcealment(opening.from, opening.to);
              addConcealment(closingLabel.from, closing.to);
              // The arrow stands in for the hidden URL, so it only shows
              // while the URL is concealed. A link saved as a source shows
              // its site's icon instead.
              if (source) {
                decorations.push(Decoration.widget({
                  widget: new SourceLinkIconWidget(target, node.from, canPreview, Boolean(place?.preview)),
                  side: -1,
                }).range(closingLabel.from));
              } else if (isBrowserUrl(target)) {
                addMarkClass(opening.to, closingLabel.from, "cm-writing-link-external");
              }
            }
          }
          return;
        }
        // A bare URL (GFM autolink) is a link in its own right: it reads the
        // same as a labelled link, arrow included.
        if (
          node.name === "URL"
          && parent
          && !["Link", "Image", "LinkReference", "Autolink"].includes(parent.name)
          && isBrowserUrl(state.sliceDoc(node.from, node.to))
        ) {
          addMarkClass(node.from, node.to, "cm-writing-link cm-writing-link-external");
          // Read like the reader shows it: site and path, no "https://".
          // The full address comes back as soon as the caret reaches it.
          if (!constructIsRevealed(state, node.from, node.to, contentFocused)) {
            const url = state.sliceDoc(node.from, node.to);
            addConcealment(node.from, node.from + hiddenUrlPrefixLength(url));
            if (url.endsWith("/")) addConcealment(node.to - 1, node.to);
          }
          return;
        }
        if (node.name === "HorizontalRule") {
          if (node.to <= frontmatterEnd) return;
          if (!lineConstructIsRevealed(state, node.from, node.from, node.to, contentFocused)) {
            addConcealment(node.from, node.to, new HorizontalRuleWidget());
          }
          return;
        }
        if (node.name === "FencedCode") {
          addFencedCodeLineClasses(
            state,
            node.from,
            node.to,
            visible.from,
            visible.to,
            addLineClass,
          );
          const fenceMarks = directChildrenNamed(node.node, "CodeMark");
          const openingFence = fenceMarks[0];
          const closingFence = fenceMarks[fenceMarks.length - 1];
          if (
            fenceMarks.length >= 2 &&
            openingFence &&
            closingFence &&
            !constructIsRevealed(state, node.from, node.to, contentFocused)
          ) {
            addConcealment(
              openingFence.from,
              state.doc.lineAt(openingFence.from).to,
            );
            addConcealment(closingFence.from, closingFence.to);
          }
          return;
        }

        if (!parent) return;

        if (node.name === "ListMark" && parent.name === "ListItem") {
          const list = parent.parent;
          const markerTo = markerWithSeparatorEnd(state, node.to, parent.to);
          const task = directChildrenNamed(parent, "Task")[0];
          addLineClass(state.doc.lineAt(node.from).from, "cm-writing-list-item");
          if (lineConstructIsRevealed(state, node.from, parent.from, parent.to, contentFocused)) {
            return;
          }
          if (task) {
            addConcealment(node.from, markerTo);
          } else if (list?.name === "BulletList") {
            addConcealment(
              node.from,
              markerTo,
              new ListMarkerWidget("bullet", "•"),
            );
          } else if (list?.name === "OrderedList") {
            addConcealment(
              node.from,
              markerTo,
              new ListMarkerWidget("ordered", state.sliceDoc(node.from, node.to)),
            );
          }
          return;
        }

        if (node.name === "TaskMarker" && parent.name === "Task") {
          if (lineConstructIsRevealed(state, node.from, parent.from, parent.to, contentFocused)) {
            return;
          }
          addConcealment(
            node.from,
            node.to,
            new TaskCheckboxWidget(
              node.from,
              node.to,
              /x/i.test(state.sliceDoc(node.from, node.to)),
              taskAccessibleLabel(
                state.sliceDoc(node.to, state.doc.lineAt(node.from).to),
              ),
            ),
          );
          return;
        }

        const blockquote =
          node.name === "QuoteMark"
            ? closestAncestorNamed(parent, "Blockquote")
            : null;
        if (blockquote) {
          const callout = parseCalloutHeader(state, blockquote);
          const line = state.doc.lineAt(node.from);
          addLineClass(
            line.from,
            callout
              ? `cm-writing-blockquote cm-writing-callout cm-writing-callout-${callout.type}`
              : "cm-writing-blockquote",
          );
          if (
            lineConstructIsRevealed(
              state,
              node.from,
              blockquote.from,
              blockquote.to,
              contentFocused,
            )
          ) {
            return;
          }
          addConcealment(
            node.from,
            markerWithSeparatorEnd(state, node.to, blockquote.to),
          );
          if (
            callout &&
            line.from === callout.lineFrom &&
            callout.markerFrom >= node.to
          ) {
            addConcealment(
              callout.markerFrom,
              callout.markerTo,
              new CalloutBadgeWidget(callout.type, callout.label),
            );
          }
          return;
        }

        if (constructIsRevealed(state, parent.from, parent.to, contentFocused)) return;

        if (node.name === "HeaderMark" && /^ATXHeading[1-6]$/.test(parent.name)) {
          if (node.from === parent.from) {
            const separator = state.sliceDoc(node.to, node.to + 1);
            const separatorLength = separator === " " || separator === "\t" ? 1 : 0;
            addConcealment(node.from, Math.min(parent.to, node.to + separatorLength));
          } else {
            addConcealment(node.from, node.to);
          }
          return;
        }

        if (
          node.name === "EmphasisMark" &&
          (parent.name === "StrongEmphasis" || parent.name === "Emphasis")
        ) {
          addConcealment(node.from, node.to);
          return;
        }

        if (node.name === "CodeMark" && parent.name === "InlineCode") {
          addConcealment(node.from, node.to);
          return;
        }

        if (node.name === "StrikethroughMark" && parent.name === "Strikethrough") {
          addConcealment(node.from, node.to);
        }
      },
    });
  }

  return {
    decorations: Decoration.set(decorations, true),
    atomic: Decoration.set(atomic, true),
  };
}

function findFrontmatterEnd(state: EditorView["state"]): number {
  if (state.doc.lines < 2 || state.doc.line(1).text !== "---") return -1;
  for (let lineNumber = 2; lineNumber <= state.doc.lines; lineNumber += 1) {
    const line = state.doc.line(lineNumber);
    if (line.text === "---" || line.text === "...") return line.to;
  }
  return -1;
}

interface CalloutHeader {
  type: string;
  label: string;
  lineFrom: number;
  markerFrom: number;
  markerTo: number;
}

function parseCalloutHeader(
  state: EditorView["state"],
  blockquote: TreeNodeLike,
): CalloutHeader | null {
  const line = state.doc.lineAt(blockquote.from);
  const match = /^\s*>\s*(\[!([A-Za-z0-9_-]+)\][+-]?)(?=\s|$)/.exec(line.text);
  if (!match) return null;
  const markerOffset = match[0].lastIndexOf(match[1]);
  const type = match[2].toLowerCase();
  return {
    type,
    label: type
      .split(/[-_]+/)
      .filter(Boolean)
      .map((part) => part[0].toUpperCase() + part.slice(1))
      .join(" "),
    lineFrom: line.from,
    markerFrom: line.from + markerOffset,
    markerTo: line.from + markerOffset + match[1].length,
  };
}

/** Where a rendered image or video loads from, or null when it stays as text. */
function mediaSource(target: string): string | null {
  const attachment = attachmentName(target);
  if (!attachment) return safeRemoteImageSource(target);
  const source = attachmentUrl(target);
  if (!source) void primeAttachmentsDir();
  return source;
}

const MEDIA_LINE = /^!\[[^\]\n]*\]\(([^)\s]+)\)\s*$/;

/**
 * Backspace on an empty line under an image or video removes both lines, so
 * one key undoes a drop. Elsewhere Backspace keeps its ordinary meaning.
 */
function deleteMediaLineAbove(view: EditorView): boolean {
  const { state } = view;
  if (state.readOnly || state.selection.ranges.length > 1) return false;
  const caret = state.selection.main;
  const line = state.doc.lineAt(caret.head);
  if (!caret.empty || line.length > 0 || line.number === 1) return false;
  const above = state.doc.line(line.number - 1);
  const target = MEDIA_LINE.exec(above.text)?.[1];
  if (!target || !mediaSource(target)) return false;
  const from = above.number > 1 ? above.from - 1 : above.from;
  view.dispatch({
    changes: { from, to: line.from },
    selection: { anchor: from },
    userEvent: "delete.backward",
    scrollIntoView: true,
  });
  return true;
}

function safeRemoteImageSource(source: string): string | null {
  try {
    const parsed = new URL(source);
    return parsed.protocol === "http:" || parsed.protocol === "https:"
      ? source
      : null;
  } catch {
    return null;
  }
}

function taskAccessibleLabel(source: string): string {
  return source
    .replace(/!\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
    .replace(/\[\[([^\]|]+)\|([^\]]+)\]\]/g, "$2")
    .replace(/\[\[([^\]]+)\]\]/g, "$1")
    .replace(/(`+)(.*?)\1/g, "$2")
    .replace(/(\*\*|__|~~)(.*?)\1/g, "$2")
    .replace(/[*_~]/g, "")
    .replace(/\s+/g, " ")
    .trim();
}

function directChildrenNamed(node: TreeNodeLike, name: string): TreeNodeLike[] {
  const matches: TreeNodeLike[] = [];
  for (let child = node.firstChild; child; child = child.nextSibling) {
    if (child.name === name) matches.push(child);
  }
  return matches;
}

function closestAncestorNamed(
  node: TreeNodeLike | null,
  name: string,
): TreeNodeLike | null {
  for (let current = node; current; current = current.parent) {
    if (current.name === name) return current;
  }
  return null;
}

function markerWithSeparatorEnd(
  state: EditorView["state"],
  markerTo: number,
  constructTo: number,
): number {
  const separator = state.sliceDoc(markerTo, markerTo + 1);
  return separator === " " || separator === "\t"
    ? Math.min(constructTo, markerTo + 1)
    : markerTo;
}

function lineConstructIsRevealed(
  state: EditorView["state"],
  linePosition: number,
  constructFrom: number,
  constructTo: number,
  contentFocused: boolean,
): boolean {
  if (!contentFocused) return false;
  const line = state.doc.lineAt(linePosition);
  return state.selection.ranges.some((range) => {
    if (range.empty) return range.head >= line.from && range.head <= line.to;
    return range.from <= constructTo && range.to >= constructFrom;
  });
}

/**
 * An image or video stays shown while the caret only touches it, so Backspace
 * after it or Delete before it removes the whole Markdown line as one unit.
 */
function mediaIsRevealed(
  state: EditorView["state"],
  constructFrom: number,
  constructTo: number,
  contentFocused: boolean,
): boolean {
  if (!contentFocused) return false;
  return state.selection.ranges.some((range) =>
    range.empty
      ? range.head > constructFrom && range.head < constructTo
      : range.from < constructTo && range.to > constructFrom,
  );
}

function revealClickedImage(event: MouseEvent, view: EditorView): boolean {
  const image = event.target instanceof Element ? event.target.closest("img.cm-writing-image") : null;
  if (!image || event.button !== 0 || event.detail > 1) return false;
  const from = view.posAtDOM(image);
  if (view.state.sliceDoc(from, from + 2) !== "![") return false;
  event.preventDefault();
  view.focus();
  view.dispatch({ selection: { anchor: from + 2 } });
  return true;
}

function constructIsRevealed(
  state: EditorView["state"],
  constructFrom: number,
  constructTo: number,
  contentFocused: boolean,
): boolean {
  if (!contentFocused) return false;
  const firstLine = state.doc.lineAt(constructFrom);
  const lastLine = state.doc.lineAt(
    Math.max(constructFrom, constructTo - 1),
  );
  return state.selection.ranges.some((range) => {
    if (range.empty) {
      return range.head >= firstLine.from && range.head <= lastLine.to;
    }
    return range.from <= constructTo && range.to >= constructFrom;
  });
}

function addFencedCodeLineClasses(
  state: EditorView["state"],
  codeFrom: number,
  codeTo: number,
  visibleFrom: number,
  visibleTo: number,
  addLineClass: (at: number, className: string) => void,
): void {
  const firstLineNumber = state.doc.lineAt(codeFrom).number;
  const lastLineNumber = state.doc.lineAt(Math.max(codeFrom, codeTo - 1)).number;
  let line = state.doc.lineAt(Math.max(codeFrom, visibleFrom));
  while (line.from < codeTo && line.from < visibleTo) {
    addLineClass(line.from, "cm-writing-fenced-code");
    if (line.number === firstLineNumber) {
      addLineClass(line.from, "cm-writing-fenced-code-first");
    }
    if (line.number === lastLineNumber) {
      addLineClass(line.from, "cm-writing-fenced-code-last");
    }
    if (line.number === state.doc.lines) break;
    line = state.doc.line(line.number + 1);
  }
}
