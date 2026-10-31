// SPDX-License-Identifier: AGPL-3.0-only
import {
  forwardRef,
  useImperativeHandle,
  useLayoutEffect,
  useRef,
} from "react";
import {
  defaultKeymap,
  history,
  historyKeymap,
  redoDepth,
  undoDepth,
} from "@codemirror/commands";
import { closeSearchPanel, findNext, findPrevious, openSearchPanel, search } from "@codemirror/search";
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState } from "@codemirror/state";
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightSpecialChars,
  keymap,
  placeholder,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { toast } from "sonner";
import { createSlashEditing, refreshSlashLabels, slashItems, type SlashLabels } from "./slashEditing";
import { mediaPaste } from "./mediaPaste";
import { setSourceLinkUrls, sourceCardMount, sourceIconMount, sourceLinkPreference, sourceLinkUrls, sourcePaste, type SourcePasteLabels } from "./sourcePaste";
import type {
  MarkdownEditorHandle,
  MarkdownEditorProps,
  MarkdownEditorStatus,
} from "./MarkdownEditor";
import { runMarkdownCommand } from "./markdownCommands";
import { indentMarkdown, markdownBrackets, outdentMarkdown } from "./markdownTyping";
import { createWikiLinkEditing, setResolvedWikiLinkTargets, setEditorReferenceContext } from "./wikiLinkEditing";
import { createLinkCompletion, refreshLinkCompletion, type LinkCompletionLabels, type LinkCompletionNote } from "./wikiLinkCompletion";
import {
  setWritingCompositionActive,
  writingPresentation,
} from "./writingPresentation";

// The writer's last choice between a saved link and its preview card, for the next paste.
const SOURCE_LINK_PREVIEW_KEY = "wenlan-source-link-preview";
const sourceLinkPreviewChoice = {
  prefer(): boolean {
    try {
      return localStorage.getItem(SOURCE_LINK_PREVIEW_KEY) === "card";
    } catch {
      return false;
    }
  },
  remember(preview: boolean): void {
    try {
      localStorage.setItem(SOURCE_LINK_PREVIEW_KEY, preview ? "card" : "link");
    } catch {
      // Not remembered; links stay links.
    }
  },
};

const sourcePreservingDefaultKeymap = defaultKeymap.filter(
  (binding) => binding.key !== "Mod-Enter" && binding.key !== "Escape",
);

// Only the find and replace keys from @codemirror/search: its go-to-line,
// select-next-occurrence and select-all-matches bindings are left out.
// Escape is handled with the editor's own Escape below, so an open panel
// closes before the editor is left.
const findKeymap = [
  { key: "Mod-f", run: openSearchPanel, scope: "editor search-panel" },
  { key: "F3", run: findNext, shift: findPrevious, scope: "editor search-panel", preventDefault: true },
  { key: "Mod-g", run: findNext, shift: findPrevious, scope: "editor search-panel", preventDefault: true },
  { key: "Escape", run: closeSearchPanel, scope: "search-panel" },
];

export interface CodeMirrorMarkdownEditorProps extends MarkdownEditorProps {
  onConstructionFailure?(): void;
}

export const CodeMirrorMarkdownEditor = forwardRef<
  MarkdownEditorHandle,
  CodeMirrorMarkdownEditorProps
>(function CodeMirrorMarkdownEditor(props, ref) {
  const { t } = useTranslation();
  const minHeight = props.minHeight ?? 300;
  const slashLabelsRef = useRef<SlashLabels>({ label: t("slashEditing.label"), ...Object.fromEntries(slashItems.map((item) => [item.id, t(`slashEditing.${item.id}`)])) } as SlashLabels);
  const linkLabelsRef = useRef<LinkCompletionLabels>(linkCompletionLabels(t));
  const linkNotesRef = useRef<readonly LinkCompletionNote[]>(props.linkNotes ?? []);
  const sourcePasteLabelsRef = useRef<SourcePasteLabels>(sourcePasteLabels(t));
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const focusRequestedRef = useRef(false);
  const disabledCompartmentRef = useRef(new Compartment());
  const attributesCompartmentRef = useRef(new Compartment());
  const searchPhrasesCompartmentRef = useRef(new Compartment());
  const compositionActiveRef = useRef(false);
  const lastStatusRef = useRef<{
    callback: ((status: MarkdownEditorStatus) => void) | undefined;
    key: string;
  } | null>(null);
  const callbacksRef = useRef({
    sessionId: props.sessionId,
    onDocumentChange: props.onDocumentChange,
    onSelectionChange: props.onSelectionChange,
    onSave: props.onSave,
    onCancel: props.onCancel,
    onConstructionFailure: props.onConstructionFailure,
    onStatusChange: props.onStatusChange,
    onReferenceActivate: props.onReferenceActivate,
    onReferencePreview: props.onReferencePreview,
    onWikiPageActivate: props.onWikiPageActivate,
    onWikiLinkPreview: props.onWikiLinkPreview,
    onWikiLinkPreviewLeave: props.onWikiLinkPreviewLeave,
    onLinkPick: props.onLinkPick,
    onSavePastedSource: props.onSavePastedSource,
    mountSourceIcon: props.mountSourceIcon,
    mountSourceCard: props.mountSourceCard,
  });

  const publishStatus = (view: EditorView): void => {
    const status: MarkdownEditorStatus = {
      sessionId: callbacksRef.current.sessionId,
      engine: "codemirror",
      ready: true,
      compositionActive: compositionActiveRef.current,
      canUndo: undoDepth(view.state) > 0,
      canRedo: redoDepth(view.state) > 0,
    };
    const callback = callbacksRef.current.onStatusChange;
    if (!callback) return;
    const key = JSON.stringify(status);
    if (
      lastStatusRef.current?.callback === callback &&
      lastStatusRef.current.key === key
    ) {
      return;
    }
    lastStatusRef.current = { callback, key };
    callback(status);
  };

  const actionsBlocked = (view: EditorView): boolean =>
    view.state.readOnly || compositionActiveRef.current || view.compositionStarted;

  const requestSave = (): boolean => {
    const view = viewRef.current;
    if (!view || actionsBlocked(view)) return false;
    callbacksRef.current.onSave(view.state.doc.toString());
    return true;
  };

  const requestCancel = (): boolean => {
    const view = viewRef.current;
    if (!view || actionsBlocked(view)) return false;
    callbacksRef.current.onCancel();
    return true;
  };

  useImperativeHandle(
    ref,
    () => ({
      focus(): void {
        focusRequestedRef.current = true;
        if (viewRef.current) {
          viewRef.current.focus();
        }
      },
      runCommand(command): boolean {
        const view = viewRef.current;
        if (!view || actionsBlocked(view)) return false;
        if (command === "find") return openSearchPanel(view);
        const applied = runMarkdownCommand(view, command);
        publishStatus(view);
        return applied;
      },
      requestSave,
      requestCancel,
    }),
    [],
  );

  useLayoutEffect(() => {
    slashLabelsRef.current = { label: t("slashEditing.label"), ...Object.fromEntries(slashItems.map((item) => [item.id, t(`slashEditing.${item.id}`)])) } as SlashLabels;
    linkLabelsRef.current = linkCompletionLabels(t);
    sourcePasteLabelsRef.current = sourcePasteLabels(t);
    viewRef.current?.dispatch({ effects: [
      refreshSlashLabels.of(null),
      refreshLinkCompletion.of(null),
      searchPhrasesCompartmentRef.current.reconfigure(EditorState.phrases.of(searchPhrases(t))),
    ] });
  }, [t]);

  useLayoutEffect(() => {
    if (props.sourceLinks) viewRef.current?.dispatch({ effects: setSourceLinkUrls.of(props.sourceLinks) });
  }, [props.sourceLinks]);

  useLayoutEffect(() => {
    linkNotesRef.current = props.linkNotes ?? [];
    viewRef.current?.dispatch({ effects: refreshLinkCompletion.of(null) });
  }, [props.linkNotes]);

  useLayoutEffect(() => {
    callbacksRef.current = {
      sessionId: props.sessionId,
      onDocumentChange: props.onDocumentChange,
      onSelectionChange: props.onSelectionChange,
      onSave: props.onSave,
      onCancel: props.onCancel,
      onConstructionFailure: props.onConstructionFailure,
      onStatusChange: props.onStatusChange,
      onReferenceActivate: props.onReferenceActivate,
      onReferencePreview: props.onReferencePreview,
      onWikiPageActivate: props.onWikiPageActivate,
      onWikiLinkPreview: props.onWikiLinkPreview,
      onWikiLinkPreviewLeave: props.onWikiLinkPreviewLeave,
      onLinkPick: props.onLinkPick,
      onSavePastedSource: props.onSavePastedSource,
      mountSourceIcon: props.mountSourceIcon,
      mountSourceCard: props.mountSourceCard,
    };
  }, [
    props.onCancel,
    props.onConstructionFailure,
    props.onDocumentChange,
    props.onSelectionChange,
    props.onSave,
    props.onStatusChange,
    props.onReferenceActivate,
    props.onReferencePreview,
    props.onWikiPageActivate,
    props.onWikiLinkPreview,
    props.onWikiLinkPreviewLeave,
    props.onLinkPick,
    props.onSavePastedSource,
    props.mountSourceIcon,
    props.mountSourceCard,
    props.sessionId,
  ]);

  useLayoutEffect(() => {
    const mount = mountRef.current;
    if (!mount) return;

    let view: EditorView;
    compositionActiveRef.current = false;
    lastStatusRef.current = null;
    try {
      view = new EditorView({
        parent: mount,
        state: EditorState.create({
          doc: props.initialDocument,
          selection: props.initialSelection && {
            anchor: Math.max(0, Math.min(Math.trunc(props.initialSelection.anchor) || 0, props.initialDocument.length)),
            head: Math.max(0, Math.min(Math.trunc(props.initialSelection.head) || 0, props.initialDocument.length)),
          },
          extensions: [
            history(),
            search({ top: true }),
            searchPhrasesCompartmentRef.current.of(EditorState.phrases.of(searchPhrases(t))),
            drawSelection(),
            dropCursor(),
            highlightSpecialChars(),
            EditorView.lineWrapping,
            markdown({
              base: markdownLanguage,
              pasteURLAsLink: false,
              completeHTMLTags: false,
            }),
            syntaxHighlighting(
              HighlightStyle.define([
                { tag: tags.heading, color: "var(--mem-accent-page)", fontWeight: "600" },
                { tag: tags.strong, color: "var(--mem-text)", fontWeight: "700" },
                { tag: tags.emphasis, color: "var(--mem-text-secondary)", fontStyle: "italic" },
                { tag: [tags.link, tags.url], color: "var(--mem-link)" },
                { tag: tags.monospace, color: "var(--mem-accent-warm)" },
                { tag: [tags.comment, tags.meta], color: "var(--mem-text-tertiary)" },
                { tag: [tags.atom, tags.keyword], color: "var(--mem-accent-sage)" },
              ]),
            ),
            EditorView.theme({
              // Tooltip wrappers inherit theme classes; keep editor layout off that portal.
              "&.cm-editor": {
                boxSizing: "border-box",
                width: "100%",
                minHeight: `${minHeight}px`,
                // Clip, not hidden: a hidden overflow makes the editor its own
                // scroll container, which pins the sticky find panel to the
                // top of the document instead of the visible pane.
                overflow: "clip",
                color: "var(--mem-text)",
                backgroundColor: props.seamless ? "transparent" : "var(--mem-detail-surface)",
                border: props.seamless ? "none" : "1px solid var(--mem-border)",
                borderRadius: props.seamless ? "0" : "var(--mem-radius-md)",
                fontSize: "var(--mem-text-md)",
              },
              "&.cm-focused": {
                outline: "none",
                borderColor: props.seamless ? "transparent" : "var(--mem-accent-page)",
              },
              ".cm-scroller": {
                minHeight: `${minHeight}px`,
                height: "auto",
                overflow: "visible",
                fontFamily:
                  "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
              },
              ".cm-content": {
                minHeight: `${minHeight}px`,
                padding: props.seamless ? "0" : "0.75rem",
                caretColor: "var(--mem-accent-indigo)",
              },
              ".cm-cursor": {
                borderLeftColor: "var(--mem-accent-indigo)",
              },
              ".cm-wiki-page-link": {
                color: "var(--mem-link)",
                textDecoration: "underline",
                textDecorationColor: "color-mix(in srgb, currentColor 45%, transparent)",
                textUnderlineOffset: "2px",
                cursor: "pointer",
              },
              ".cm-wiki-page-link:hover": {
                color: "var(--mem-link-hover)",
              },
              // Syntax highlighting wraps the marker text (the "1" of [1]) in
              // its own link-coloured span; let the reference decide.
              ".cm-wiki-page-link *": {
                color: "inherit",
              },
              // Citation markers are footnotes, as in reading mode: grey
              // until pointed at, so they never outrank the links.
              ".cm-wiki-page-link.cm-citation-link": {
                color: "var(--mem-text-tertiary)",
              },
              ".cm-wiki-page-link.cm-citation-link:not(.reference-link--unverified):is(:hover, :focus-visible)": {
                color: "var(--mem-link)",
              },
              ".cm-wiki-page-link:focus-visible": {
                outline: "2px solid var(--mem-accent-indigo)",
                outlineOffset: "1px",
                borderRadius: "2px",
              },
              ".cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection": {
                backgroundColor: "var(--mem-indigo-bg)",
              },
              // Find and replace panel, drawn in the app's colours instead of
              // the package's light grey. "&.cm-editor" outranks its base theme.
              "&.cm-editor .cm-panels": {
                backgroundColor: "var(--mem-detail-surface)",
                color: "var(--mem-text)",
              },
              "&.cm-editor .cm-panels-top": {
                borderBottom: "1px solid var(--mem-border)",
                marginBottom: "12px",
              },
              "&.cm-editor .cm-panel.cm-search": {
                padding: "6px 28px 6px 0",
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-sm)",
              },
              "&.cm-editor .cm-search input, &.cm-editor .cm-search button, &.cm-editor .cm-search label": {
                margin: "2px 6px 2px 0",
              },
              "&.cm-editor .cm-search .cm-textfield": {
                width: "12em",
                maxWidth: "100%",
                fontSize: "var(--mem-text-sm)",
                padding: "3px 8px",
                color: "var(--mem-text)",
                backgroundColor: "var(--mem-surface)",
                border: "1px solid var(--mem-control-border)",
                borderRadius: "var(--mem-radius-sm)",
              },
              "&.cm-editor .cm-search .cm-textfield:focus": {
                outline: "2px solid var(--mem-focus-ring)",
                outlineOffset: "-1px",
              },
              "&.cm-editor .cm-search .cm-button": {
                whiteSpace: "nowrap",
                fontSize: "var(--mem-text-sm)",
                padding: "3px 10px",
                color: "var(--mem-text)",
                backgroundImage: "none",
                backgroundColor: "var(--mem-hover)",
                border: "1px solid var(--mem-control-border)",
                borderRadius: "var(--mem-radius-sm)",
                cursor: "pointer",
              },
              "&.cm-editor .cm-search .cm-button:hover": {
                backgroundColor: "var(--mem-hover-strong)",
              },
              "&.cm-editor .cm-search label": {
                marginInlineEnd: "10px",
                fontSize: "var(--mem-text-sm)",
                color: "var(--mem-text-secondary)",
                whiteSpace: "nowrap",
              },
              "&.cm-editor .cm-search input[type=checkbox]": {
                accentColor: "var(--mem-accent-indigo)",
                verticalAlign: "middle",
              },
              "&.cm-editor .cm-search [name=close]": {
                top: "6px",
                right: "4px",
                width: "22px",
                height: "22px",
                fontSize: "18px",
                lineHeight: "20px",
                color: "var(--mem-text-secondary)",
                cursor: "pointer",
                borderRadius: "var(--mem-radius-sm)",
              },
              "&.cm-editor .cm-search [name=close]:hover": {
                color: "var(--mem-text)",
                backgroundColor: "var(--mem-hover)",
              },
              "&.cm-editor .cm-searchMatch": {
                backgroundColor: "color-mix(in srgb, var(--mem-accent-amber) 28%, transparent)",
                borderRadius: "2px",
              },
              "&.cm-editor .cm-searchMatch-selected": {
                backgroundColor: "color-mix(in srgb, var(--mem-accent-amber) 60%, transparent)",
                outline: "1px solid var(--mem-accent-amber)",
              },
            }),
            writingPresentation(),
            createSlashEditing({ labels: () => slashLabelsRef.current, blocked: actionsBlocked }).extension,
            mediaPaste({
              blocked: actionsBlocked,
              onError: (name) => toast.error(name
                ? t("pageAttachments.failed", { name })
                : t("pageAttachments.imageFailed")),
              pendingLabel: (name) => name ? t("pageAttachments.adding", { name }) : t("pageAttachments.addingImage"),
            }),
            sourceLinkUrls.init(() => props.sourceLinks ?? new Map()),
            sourceIconMount.of((container, url, place) => callbacksRef.current.mountSourceIcon?.(container, url, place) ?? (() => undefined)),
            sourceCardMount.of((container, url, title, place) => callbacksRef.current.mountSourceCard?.(container, url, title, place) ?? (() => undefined)),
            sourceLinkPreference.of(sourceLinkPreviewChoice),
            sourcePaste({
              blocked: actionsBlocked,
              enabled: () => Boolean(callbacksRef.current.onSavePastedSource),
              labels: () => sourcePasteLabelsRef.current,
              save: (url, label) => {
                const save = callbacksRef.current.onSavePastedSource;
                return save ? save(url, label) : Promise.reject(new Error("unavailable"));
              },
            }),
            props.linkNotes ? createLinkCompletion({
              notes: () => linkNotesRef.current,
              labels: () => linkLabelsRef.current,
              blocked: actionsBlocked,
              onPick: (note, asSource) => callbacksRef.current.onLinkPick?.(note, asSource),
            }).extension : [],
            createWikiLinkEditing(props.wikiLinkTargets ?? new Map(), {
              onActivate: (pageId, anchor) => callbacksRef.current.onWikiPageActivate?.(pageId, anchor),
              onPreview: (pageId, anchor, keyboard) => callbacksRef.current.onWikiLinkPreview?.(pageId, anchor, keyboard),
              onPreviewLeave: () => callbacksRef.current.onWikiLinkPreviewLeave?.(),
              onReferenceActivate: (target, anchor) => callbacksRef.current.onReferenceActivate?.(target, anchor),
              onReferencePreview: (target, anchor, keyboard) => callbacksRef.current.onReferencePreview?.(target, anchor, keyboard),
            }, props.referenceContext),
            EditorView.domEventHandlers({
              compositionstart: (_event, currentView) => {
                if (viewRef.current !== currentView) return false;
                compositionActiveRef.current = true;
                currentView.dispatch({
                  effects: setWritingCompositionActive.of(true),
                });
                publishStatus(currentView);
                return false;
              },
              compositionend: (_event, currentView) => {
                if (viewRef.current !== currentView) return false;
                Promise.resolve().then(() => {
                  if (
                    viewRef.current !== currentView ||
                    currentView.compositionStarted
                  ) {
                    return;
                  }
                  compositionActiveRef.current = false;
                  currentView.dispatch({
                    effects: setWritingCompositionActive.of(false),
                  });
                  publishStatus(currentView);
                });
                return false;
              },
              // Focus loss ends a composition in WebView2 and WebKit, even
              // when compositionend is not delivered; keep the preedit as text.
              blur: (_event, currentView) => {
                if (viewRef.current !== currentView || !compositionActiveRef.current) return false;
                compositionActiveRef.current = false;
                currentView.dispatch({
                  effects: setWritingCompositionActive.of(false),
                });
                publishStatus(currentView);
                return false;
              },
            }),
            keymap.of([
              ...([ ["Mod-b", "bold"], ["Mod-i", "italic"] ] as const).map(([key, command]) => ({
                key,
                run: (currentView: EditorView) => {
                  if (currentView.state.readOnly) return true;
                  if (actionsBlocked(currentView)) return false;
                  return runMarkdownCommand(currentView, command);
                },
              })),
              {
                key: "Mod-Enter",
                run: (currentView) => {
                  if (currentView.state.readOnly) return true;
                  if (actionsBlocked(currentView)) return false;
                  requestSave();
                  return true;
                },
              },
              {
                // Tab nests a list item, Shift+Tab moves it out; on other
                // lines Tab indents. Escape, then Tab, still leaves the editor.
                key: "Tab",
                run: (currentView) => {
                  if (actionsBlocked(currentView)) return false;
                  return indentMarkdown(currentView);
                },
                shift: (currentView) => {
                  if (actionsBlocked(currentView)) return false;
                  return outdentMarkdown(currentView);
                },
              },
              {
                key: "Escape",
                run: (currentView) => {
                  if (compositionActiveRef.current || currentView.compositionStarted) return false;
                  // CodeMirror's escape hatch: a Tab within two seconds of
                  // Escape moves focus instead of indenting. Bound keys do not
                  // reach the view's own Escape handler, so set it here.
                  currentView.setTabFocusMode(2000);
                  // An open find panel closes first; the next Escape leaves.
                  if (closeSearchPanel(currentView)) return true;
                  if (currentView.state.readOnly) return true;
                  requestCancel();
                  return true;
                },
              },
            ]),
            markdownBrackets(),
            keymap.of(sourcePreservingDefaultKeymap),
            keymap.of(historyKeymap),
            keymap.of(findKeymap),
            EditorView.updateListener.of((update) => {
              if (update.docChanged) {
                callbacksRef.current.onDocumentChange(update.state.doc.toString());
              }
              if (update.selectionSet || update.docChanged) {
                const { anchor, head } = update.state.selection.main;
                callbacksRef.current.onSelectionChange?.({ anchor, head });
              }
              publishStatus(update.view);
            }),
            disabledCompartmentRef.current.of(disabledExtensions(props.disabled)),
            attributesCompartmentRef.current.of(contentAttributes(props)),
          ],
        }),
      });
    } catch {
      mount.replaceChildren();
      callbacksRef.current.onConstructionFailure?.();
      return;
    }

    viewRef.current = view;
    publishStatus(view);
    if (focusRequestedRef.current) view.focus();
    return () => {
      if (viewRef.current === view) viewRef.current = null;
      compositionActiveRef.current = false;
      view.destroy();
      mount.replaceChildren();
    };
    // EditorView lifetime is intentionally keyed only by the edit session.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.sessionId]);

  useLayoutEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({ effects: [setResolvedWikiLinkTargets(props.wikiLinkTargets ?? new Map()), setEditorReferenceContext(props.referenceContext ?? {})] });
  }, [props.wikiLinkTargets, props.referenceContext, props.sessionId]);

  useLayoutEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: disabledCompartmentRef.current.reconfigure(disabledExtensions(props.disabled)),
    });
  }, [props.disabled, props.sessionId]);

  useLayoutEffect(() => {
    const view = viewRef.current;
    if (!view) return;
    view.dispatch({
      effects: attributesCompartmentRef.current.reconfigure(contentAttributes(props)),
    });
  }, [props.ariaLabel, props.describedBy, props.disabled, props.placeholder, props.sessionId]);

  return (
    <div
      ref={mountRef}
      data-markdown-editor-engine="codemirror"
      style={{ width: "100%", minHeight: `${minHeight}px` }}
      onKeyDownCapture={(event) => {
        if (
          (compositionActiveRef.current || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229)
          && ["Enter", "Escape", "ArrowDown", "ArrowUp", "Tab"].includes(event.key)
        ) {
          // Leave the browser's IME default intact while keeping candidate
          // keys away from CodeMirror's ordinary Enter/Escape keymaps.
          event.stopPropagation();
          return;
        }
        if (
          (event.metaKey || event.ctrlKey) &&
          event.key.toLowerCase() === "s"
        ) {
          event.preventDefault();
          requestSave();
        }
      }}
    />
  );
});

function disabledExtensions(disabled: boolean) {
  return [EditorState.readOnly.of(disabled), EditorView.editable.of(!disabled)];
}

function contentAttributes(props: Pick<MarkdownEditorProps, "ariaLabel" | "describedBy" | "disabled" | "placeholder">) {
  return [
    EditorView.contentAttributes.of({
      "aria-label": props.ariaLabel,
      "aria-multiline": "true",
      "aria-disabled": String(props.disabled),
      ...(props.describedBy ? { "aria-describedby": props.describedBy } : {}),
    }),
    props.placeholder ? placeholder(props.placeholder) : [],
  ];
}

function linkCompletionLabels(t: TFunction): LinkCompletionLabels {
  return {
    label: t("linkCompletion.label"),
    empty: t("linkCompletion.empty"),
    useAsSource: t("noteBasis.useAsSource"),
    useAsSourceHint: t("noteBasis.useAsSourceHint"),
  };
}

/** @codemirror/search reads its labels and screen-reader announcements as phrases. */
function searchPhrases(t: TFunction): Record<string, string> {
  return {
    Find: t("editorSearch.find"),
    Replace: t("editorSearch.replace"),
    next: t("editorSearch.next"),
    previous: t("editorSearch.previous"),
    all: t("editorSearch.selectAll"),
    "match case": t("editorSearch.matchCase"),
    regexp: t("editorSearch.regexp"),
    "by word": t("editorSearch.wholeWord"),
    replace: t("editorSearch.replace"),
    "replace all": t("editorSearch.replaceAll"),
    close: t("common.close"),
    "current match": t("editorSearch.currentMatch"),
    "on line": t("editorSearch.onLine"),
    "replaced match on line $": t("editorSearch.replacedOne"),
    "replaced $ matches": t("editorSearch.replacedMany"),
  };
}

function sourcePasteLabels(t: TFunction): SourcePasteLabels {
  return {
    saving: t("sourcePaste.saving"),
    savedLinkOnly: t("sourcePaste.savedLinkOnly"),
    unreadable: t("sourcePaste.unreadable"),
    failed: t("sourcePaste.failed"),
    retry: t("sourcePaste.retry"),
  };
}
