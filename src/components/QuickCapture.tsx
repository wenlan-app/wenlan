// SPDX-License-Identifier: AGPL-3.0-only
import { useState, useRef, useEffect } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { quickCapture } from "../lib/tauri";
import { isDaemonUnreachableError } from "../lib/setupErrors";
import { isInputCompositionActive } from "../lib/inputComposition";
import { confirmAction } from "../lib/confirmAction";
import { primaryShortcutModifier } from "../lib/windowChrome";
import { MarkdownEditor, type MarkdownEditorHandle } from "./memory/editor/MarkdownEditor";
import { useEditorLinkNotes } from "./memory/editor/useEditorLinkNotes";
import {
  clearQuickCaptureDraft,
  quickCaptureDraftKey,
  readQuickCaptureDraft,
  writeQuickCaptureDraft,
} from "./quickCaptureDraftRecovery";

interface QuickCaptureProps {
  isOpen: boolean;
  onClose: () => void;
  standalone?: boolean;
}

/** Thrown when the daemon answers but stored nothing; keeps the draft. */
class NothingSavedError extends Error {}

export default function QuickCapture({ isOpen, onClose, standalone }: QuickCaptureProps) {
  const { t } = useTranslation();
  const [draftKey] = useState(() => quickCaptureDraftKey(standalone));
  // Text left from before a restart, crash or reload comes back as typed.
  const [recovered] = useState(() => readQuickCaptureDraft(draftKey));
  const [content, setContent] = useState(recovered.content);
  // True while this device cannot keep a copy of the text.
  const [backupFailed, setBackupFailed] = useState(recovered.failed);
  const [saved, setSaved] = useState(false);
  const [tooShort, setTooShort] = useState(false);
  const [composing, setComposing] = useState(false);
  const [fallback, setFallback] = useState(false);
  // A new editor session starts empty after a save or a discard.
  const [session, setSession] = useState(0);
  const editorRef = useRef<MarkdownEditorHandle>(null);
  // The editor reports every change; the refs answer key handlers at once,
  // before React re-renders.
  const documentRef = useRef(recovered.content);
  const savingRef = useRef(false);
  const savedRef = useRef(false);
  const composingRef = useRef(false);
  const confirmingRef = useRef(false);
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const queryClient = useQueryClient();
  // Page names load only once someone types [[.
  const linkNotes = useEditorLinkNotes(null, "", isOpen && content.includes("[["));

  useEffect(() => () => {
    if (closeTimerRef.current !== null) clearTimeout(closeTimerRef.current);
  }, []);

  const startNewSession = () => {
    if (closeTimerRef.current !== null) clearTimeout(closeTimerRef.current);
    closeTimerRef.current = null;
    savedRef.current = false;
    setSaved(false);
    documentRef.current = "";
    setContent("");
    setTooShort(false);
    setSession((value) => value + 1);
  };

  const captureMutation = useMutation({
    mutationFn: async (request: Parameters<typeof quickCapture>[0]) => {
      const chunks = await quickCapture(request);
      if (!(chunks > 0)) throw new NothingSavedError();
      return chunks;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["indexedFiles"] });
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      setSaved(true);
      savedRef.current = true;
      // The memory is saved. A copy that cannot be removed means storage is
      // blocked, so it cannot come back either; closing goes ahead.
      clearQuickCaptureDraft(draftKey);
      closeTimerRef.current = setTimeout(() => {
        startNewSession();
        onClose();
      }, 500);
    },
    onSettled: () => {
      savingRef.current = false;
    },
  });

  // The daemon's own words follow, as other inline save errors in the app
  // show them. Nothing here clears the text, so a failed save keeps it in the
  // editor and in this device's copy for another try.
  // An unreachable daemon gets a plain sentence: its transport text
  // ("error sending request for url ...") tells the person nothing.
  const saveFailure = captureMutation.isError
    ? captureMutation.error instanceof NothingSavedError
      ? t("quickCapture.nothingSaved")
      : isDaemonUnreachableError(captureMutation.error)
        ? t("quickCapture.daemonUnreachable")
        : t("quickCapture.saveError", {
          message: captureMutation.error instanceof Error
            ? captureMutation.error.message
            : String(captureMutation.error),
        })
    : null;

  const handleSubmit = (body = documentRef.current) => {
    if (savingRef.current || savedRef.current || composingRef.current || confirmingRef.current) return;
    if (!body.trim()) return;
    if (Array.from(body.trim()).length < 10) {
      setTooShort(true);
      return;
    }
    savingRef.current = true;
    // Send the body exactly as typed: leading and trailing whitespace can be
    // Markdown (an indented code block, a hard line break).
    captureMutation.mutate({ content: body });
  };

  const requestClose = async () => {
    if (savingRef.current || composingRef.current || confirmingRef.current) return;
    if (!savedRef.current && documentRef.current.trim()) {
      confirmingRef.current = true;
      let discard = false;
      try {
        discard = await confirmAction(t("quickCapture.discardConfirm"), {
          confirm: t("quickCapture.discard"),
          cancel: t("quickCapture.keepWriting"),
        });
      } finally {
        confirmingRef.current = false;
      }
      if (!discard) {
        editorRef.current?.focus();
        return;
      }
      // A save or an IME composition may have started while the question was open.
      if (savingRef.current || composingRef.current) return;
    }
    clearQuickCaptureDraft(draftKey);
    startNewSession();
    onClose();
  };

  // Document-level keydown so Esc / Cmd+Enter also work when focus has left
  // the editor (the close button, or a window shown again after hiding). The
  // editor handles both keys itself, so its own events are skipped here.
  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented) return;
      // Escape and Enter while the IME composes pick or drop its candidate.
      if (e.isComposing || e.keyCode === 229 || composingRef.current || isInputCompositionActive()) return;
      if (e.target instanceof Element && e.target.closest("[data-markdown-editor-engine], .page-editor-native-surface")) return;
      if (e.key === "Escape") {
        e.preventDefault();
        void requestClose();
      } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        handleSubmit();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
    // Both handlers read refs; re-binding on content keeps their closures fresh.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, onClose, content]);

  // Re-focus the editor whenever the window becomes visible.
  // In standalone (Tauri window) mode, hiding the window doesn't unmount the
  // component — it just hides the webview. When it comes back via
  // `WebviewWindow.show()` the document fires `visibilitychange`; we use that
  // to restore text-entry focus so the user can type immediately.
  useEffect(() => {
    if (!isOpen) return;
    const refocus = () => {
      if (!document.hidden && !confirmingRef.current && editorRef.current) {
        editorRef.current.focus();
      }
    };
    refocus(); // initial mount + any state toggle
    document.addEventListener("visibilitychange", refocus);
    window.addEventListener("focus", refocus);
    return () => {
      document.removeEventListener("visibilitychange", refocus);
      window.removeEventListener("focus", refocus);
    };
  }, [isOpen]);

  if (!isOpen) return null;

  const charCount = Array.from(content.trim()).length;
  const isEmpty = charCount === 0;
  const isPending = captureMutation.isPending;

  // The whole border takes the state colour.
  const borderAccent = saved
    ? "var(--mem-accent-warm)"
    : isPending
      ? "var(--mem-accent-amber)"
      : !isEmpty
        ? "var(--mem-accent-indigo)"
        : "var(--mem-border)";

  const card = (
    <div
      data-testid="quick-capture-card"
      className="flex-1 flex flex-col overflow-hidden rounded-xl"
      style={{
        backgroundColor: "var(--mem-surface)",
        border: "1px solid var(--mem-border)",
        borderColor: borderAccent,
        // Keep the standalone window flat, including macOS. Only the in-app
        // modal paints a shadow over its backdrop; state still uses the border.
        boxShadow: standalone
          ? "none"
          : !isEmpty && !saved
            ? "0 0 20px var(--mem-shimmer-color), 0 8px 32px rgba(0,0,0,0.25)"
            : "0 8px 32px rgba(0,0,0,0.25)",
        transition: "border-color 0.3s ease, box-shadow 0.3s ease",
      }}
    >
      {/* Header */}
      <div
        className="flex items-center justify-between px-4 pt-3 pb-1 shrink-0"
        style={{ opacity: 0.7 }}
      >
        <div className="flex items-center gap-2">
          <div
            className={`w-1.5 h-1.5 rounded-full transition-colors duration-200 ${
              saved
                ? "bg-[var(--mem-accent-warm)]"
                : isPending
                  ? "bg-[var(--mem-accent-amber)] animate-pulse"
                  : !isEmpty
                    ? "bg-[var(--mem-accent-indigo)]"
                    : ""
            }`}
            style={isEmpty && !saved && !isPending ? { backgroundColor: "var(--mem-text-tertiary)", opacity: 0.4 } : undefined}
          />
          <span
            style={{
              fontFamily: "var(--mem-font-mono)",
              fontSize: "10px",
              fontWeight: 600,
              letterSpacing: "0.05em",
              color: "var(--mem-text-tertiary)",
              textTransform: "uppercase" as const,
            }}
          >
            {saved ? t("quickCapture.savedToMemory") : isPending ? t("quickCapture.saving") : t("quickCapture.title")}
          </span>
        </div>
        <button
          onClick={() => void requestClose()}
          disabled={isPending || composing}
          className="p-0.5 rounded transition-colors duration-150 hover:bg-[var(--mem-hover-strong)]"
          style={{ color: "var(--mem-text-tertiary)" }}
          title={t("quickCapture.closeTitle")}
        >
          <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
            <path strokeLinecap="round" strokeLinejoin="round" strokeWidth={2} d="M6 18L18 6M6 6l12 12" />
          </svg>
        </button>
      </div>

      {/* Writing surface */}
      <div className="flex-1 flex flex-col px-4 pb-3 min-h-0">
        <div className="quick-capture-editor flex-1 min-h-0 overflow-y-auto pt-2">
          {fallback && (
            <p role="status" className="quick-capture-note">{t("pageDetail.editor.fallbackTitle")}</p>
          )}
          <MarkdownEditor
            key={session}
            ref={editorRef}
            sessionId={`quick-capture:${session}`}
            initialDocument={content}
            seamless
            minHeight={120}
            disabled={isPending || saved}
            ariaLabel={t("quickCapture.title")}
            placeholder={t("quickCapture.placeholder")}
            linkNotes={linkNotes}
            onDocumentChange={(next) => {
              documentRef.current = next;
              setContent(next);
              setBackupFailed(!writeQuickCaptureDraft(draftKey, next));
              setTooShort(false);
              if (captureMutation.isError) captureMutation.reset();
            }}
            onSave={handleSubmit}
            onCancel={() => void requestClose()}
            onFallback={() => setFallback(true)}
            onStatusChange={(status) => {
              composingRef.current = status.compositionActive;
              setComposing(status.compositionActive);
            }}
          />
        </div>

        {(saveFailure || tooShort || backupFailed) && (
          <div role="alert" className="quick-capture-alert shrink-0">
            {tooShort && <p>{t("quickCapture.tooShort")}</p>}
            {saveFailure && <p>{saveFailure}</p>}
            {backupFailed ? (
              <p>{t("quickCapture.backupFailed")}</p>
            ) : saveFailure ? (
              <p className="quick-capture-alert-note">{t("quickCapture.draftKept")}</p>
            ) : null}
          </div>
        )}

        {/* Bottom bar */}
        <div
          className="flex items-center justify-between pt-2 mt-auto shrink-0"
          style={{ opacity: 0.7 }}
        >
          <span
            className="transition-opacity duration-150"
            style={{
              fontFamily: "var(--mem-font-mono)",
              fontSize: "10px",
              color: "var(--mem-text-tertiary)",
              opacity: isEmpty ? 0 : 1,
            }}
          >
            {t("quickCapture.chars", { count: charCount })}
          </span>
          <div className="flex items-center gap-3">
            <span
              className="select-none whitespace-nowrap"
              style={{
                fontFamily: "var(--mem-font-mono)",
                fontSize: "10px",
                color: "var(--mem-text-tertiary)",
                opacity: 0.5,
              }}
            >
              {t(standalone ? "quickCapture.standaloneShortcut" : "quickCapture.saveShortcut", {
                modifier: primaryShortcutModifier(),
              })}
            </span>
            <button
              onClick={() => handleSubmit()}
              disabled={isEmpty || isPending || saved || composing}
              className="whitespace-nowrap transition-all duration-200"
              style={{
                fontFamily: "var(--mem-font-body)",
                fontSize: "12px",
                fontWeight: 500,
                padding: "4px 14px",
                borderRadius: 8,
                color:
                  saved
                    ? "var(--mem-accent-warm)"
                    : isEmpty || isPending
                      ? "var(--mem-text-tertiary)"
                      : "white",
                backgroundColor:
                  saved
                    ? "var(--mem-confirm-bg)"
                    : isEmpty || isPending
                      ? "var(--mem-hover)"
                      : "var(--mem-accent-indigo)",
                cursor: isEmpty || isPending || saved ? "default" : "pointer",
                boxShadow:
                  !isEmpty && !isPending && !saved
                    ? "0 2px 10px var(--mem-shimmer-color)"
                    : "none",
              }}
            >
              {saved ? t("quickCapture.saved") : isPending ? "..." : t("quickCapture.save")}
            </button>
          </div>
        </div>
      </div>
    </div>
  );

  if (standalone) {
    // Without an exterior shadow, the card fills the window without an inset.
    return (
      <div
        className="w-full h-screen flex flex-col"
        style={{ background: "transparent", padding: 0 }}
      >
        {card}
      </div>
    );
  }

  return (
    <div
      className="fixed inset-0 flex items-center justify-center z-50"
      onClick={() => void requestClose()}
      style={{
        backgroundColor: "rgba(0,0,0,0.5)",
        backdropFilter: "blur(4px)",
        WebkitBackdropFilter: "blur(4px)",
        animation: "qc-overlay-in 0.15s ease-out",
      }}
    >
      <div
        className="w-[560px] max-w-[calc(100vw-2rem)] h-[360px] max-h-[80vh] flex flex-col"
        onClick={(e) => e.stopPropagation()}
        style={{ animation: "mem-fade-up 0.2s cubic-bezier(0.16, 1, 0.3, 1)" }}
      >
        {card}
      </div>
      <style>{`
        @keyframes qc-overlay-in {
          from { opacity: 0; }
          to { opacity: 1; }
        }
      `}</style>
    </div>
  );
}
