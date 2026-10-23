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
import { markdown, markdownLanguage } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState } from "@codemirror/state";
import {
  drawSelection,
  dropCursor,
  EditorView,
  highlightSpecialChars,
  keymap,
} from "@codemirror/view";
import { tags } from "@lezer/highlight";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { toast } from "sonner";
import { createSlashEditing, refreshSlashLabels, slashItems, type SlashLabels } from "./slashEditing";
import { mediaPaste } from "./mediaPaste";
import type {
  MarkdownEditorHandle,
  MarkdownEditorProps,
  MarkdownEditorStatus,
} from "./MarkdownEditor";
import { runMarkdownCommand } from "./markdownCommands";
import { createWikiLinkEditing, setResolvedWikiLinkTargets, setEditorReferenceContext } from "./wikiLinkEditing";
import { createLinkCompletion, refreshLinkCompletion, type LinkCompletionLabels, type LinkCompletionNote } from "./wikiLinkCompletion";
import {
  setWritingCompositionActive,
  writingPresentation,
} from "./writingPresentation";

const sourcePreservingDefaultKeymap = defaultKeymap.filter(
  (binding) => binding.key !== "Mod-Enter" && binding.key !== "Escape",
);

export interface CodeMirrorMarkdownEditorProps extends MarkdownEditorProps {
  onConstructionFailure?(): void;
}

export const CodeMirrorMarkdownEditor = forwardRef<
  MarkdownEditorHandle,
  CodeMirrorMarkdownEditorProps
>(function CodeMirrorMarkdownEditor(props, ref) {
  const { t } = useTranslation();
  const slashLabelsRef = useRef<SlashLabels>({ label: t("slashEditing.label"), ...Object.fromEntries(slashItems.map((item) => [item.id, t(`slashEditing.${item.id}`)])) } as SlashLabels);
  const linkLabelsRef = useRef<LinkCompletionLabels>(linkCompletionLabels(t));
  const linkNotesRef = useRef<readonly LinkCompletionNote[]>(props.linkNotes ?? []);
  const mountRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const focusRequestedRef = useRef(false);
  const disabledCompartmentRef = useRef(new Compartment());
  const attributesCompartmentRef = useRef(new Compartment());
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
    viewRef.current?.dispatch({ effects: [refreshSlashLabels.of(null), refreshLinkCompletion.of(null)] });
  }, [t]);

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
                minHeight: "300px",
                overflow: "hidden",
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
                minHeight: "300px",
                height: "auto",
                overflow: "visible",
                fontFamily:
                  "ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace",
              },
              ".cm-content": {
                minHeight: "300px",
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
            }),
            writingPresentation(),
            createSlashEditing({ labels: () => slashLabelsRef.current, blocked: actionsBlocked }).extension,
            mediaPaste({
              blocked: actionsBlocked,
              onError: (file) => toast.error(t("pageAttachments.failed", { name: file.name || "file" })),
              pendingLabel: (file) => t("pageAttachments.adding", { name: file.name || "file" }),
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
                key: "Escape",
                run: (currentView) => {
                  if (currentView.state.readOnly) return true;
                  if (actionsBlocked(currentView)) return false;
                  requestCancel();
                  return true;
                },
              },
            ]),
            keymap.of(sourcePreservingDefaultKeymap),
            keymap.of(historyKeymap),
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
  }, [props.ariaLabel, props.describedBy, props.disabled, props.sessionId]);

  return (
    <div
      ref={mountRef}
      data-markdown-editor-engine="codemirror"
      style={{ width: "100%", minHeight: "300px" }}
      onKeyDownCapture={(event) => {
        if (
          (compositionActiveRef.current || event.nativeEvent.isComposing || event.nativeEvent.keyCode === 229)
          && ["Enter", "Escape", "ArrowDown", "ArrowUp"].includes(event.key)
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

function contentAttributes(props: Pick<MarkdownEditorProps, "ariaLabel" | "describedBy" | "disabled">) {
  return EditorView.contentAttributes.of({
    "aria-label": props.ariaLabel,
    "aria-multiline": "true",
    "aria-disabled": String(props.disabled),
    ...(props.describedBy ? { "aria-describedby": props.describedBy } : {}),
  });
}

function linkCompletionLabels(t: TFunction): LinkCompletionLabels {
  return {
    label: t("linkCompletion.label"),
    empty: t("linkCompletion.empty"),
    useAsSource: t("noteBasis.useAsSource"),
    useAsSourceHint: t("noteBasis.useAsSourceHint"),
  };
}
