// SPDX-License-Identifier: AGPL-3.0-only
import { useState, useRef, useEffect } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { useEditorLinkNotes } from "./memory/editor/useEditorLinkNotes";
import { MarkdownEditor, type MarkdownEditorHandle } from "./memory/editor/MarkdownEditor";
import { confirmAction } from "../lib/confirmAction";
import { primaryShortcutModifier } from "../lib/windowChrome";
import { quickCapture } from "../lib/tauri";
import { quickCaptureDraftKey, readQuickCaptureDraft, writeQuickCaptureDraft, clearQuickCaptureDraft } from "./quickCaptureDraftRecovery";

interface QuickCaptureProps {
  isOpen: boolean;
  onClose: () => void;
  standalone?: boolean;
}

export default function QuickCapture({ isOpen, onClose, standalone }: QuickCaptureProps) {
  const { t } = useTranslation();
  const [draftKey] = useState(() => quickCaptureDraftKey(standalone));
  const [recovered] = useState(() => readQuickCaptureDraft(draftKey));
  const [content, setContent] = useState(recovered.content);
  const [recoveryError, setRecoveryError] = useState<"backup" | "cleanup" | null>(recovered.failed ? "backup" : null);
  const closeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  useEffect(() => () => { if (closeTimerRef.current !== null) clearTimeout(closeTimerRef.current); }, []);
  const [saved, setSaved] = useState(false);
  const [tooShort, setTooShort] = useState(false);
  const editorRef = useRef<MarkdownEditorHandle>(null);
  const documentRef = useRef(recovered.content);
  const savingRef = useRef(false);
  const savedRef = useRef(false);
  const composingRef = useRef(false);
  const [composing, setComposing] = useState(false);
  const [fallback, setFallback] = useState(false);
  const [session, setSession] = useState(0);
  const queryClient = useQueryClient();
  const linkNotes = useEditorLinkNotes(null, "", isOpen && content.includes("[["));

  const captureMutation = useMutation({
    mutationFn: async (request: Parameters<typeof quickCapture>[0]) => {
      const chunks = await quickCapture(request);
      if (!(chunks > 0)) throw new Error(t("quickCapture.noChunksSaved"));
      return chunks;
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["indexedFiles"] });
      queryClient.invalidateQueries({ queryKey: ["memories"] });
      setSaved(true);
      savedRef.current = true;
      if (!clearQuickCaptureDraft(draftKey)) { setRecoveryError("cleanup"); return; }
      setRecoveryError(null);
      closeTimerRef.current = setTimeout(() => {
        closeTimerRef.current = null;
        setSaved(false);
        savedRef.current = false;
        documentRef.current = "";
        setContent("");
        setSession(value => value + 1);
        onClose();
      }, 500);
    },
    onSettled: () => { savingRef.current = false; },
  });

  // Surfaced verbatim below the textarea on rejection -- same policy other
  // inline mutation errors in the app use. The draft stays in `content`
  // either way: nothing here clears it, so a rejected capture (e.g. the
  // quality gate's "too short" floor) keeps the user's text for a retry.
  const errorMessage = captureMutation.isError
    ? captureMutation.error instanceof Error
      ? captureMutation.error.message
      : String(captureMutation.error)
    : null;

  const handleSubmit = (document = documentRef.current) => {
    if (!document.trim() || savingRef.current || savedRef.current || composingRef.current) return;
    if (Array.from(document.trim()).length < 10) {
      setTooShort(true);
      return;
    }
    savingRef.current = true;
    captureMutation.mutate({ content: document });
  };

  const requestClose = async () => {
    if (savingRef.current || composingRef.current) return;
    if (!savedRef.current && documentRef.current.trim()) {
      if (!await confirmAction(t("pageDetail.editor.discardConfirm"), { confirm: t("quickCapture.discard"), cancel: t("common.cancel") })) return;
      // The modal can remain open while a save or IME operation begins.
      if (savingRef.current || composingRef.current) return;
    }
    if (!clearQuickCaptureDraft(draftKey)) { setRecoveryError("cleanup"); return; }
    setRecoveryError(null);
    if (closeTimerRef.current !== null) clearTimeout(closeTimerRef.current);
    closeTimerRef.current = null;
    savedRef.current = false; setSaved(false);
    documentRef.current = ""; setContent(""); setSession(value => value + 1);
    onClose();
  };

  // Document-level keydown so Esc / Cmd+Enter work regardless of focus target.
  // The textarea's onKeyDown used to own this, which broke after the window was
  // hidden and re-shown: the textarea lost focus and never regained it (because
  // `isOpen` is constant-true in standalone mode, so the auto-focus effect
  // never re-ran), leaving Esc unhandled.
  useEffect(() => {
    if (!isOpen) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing || e.keyCode === 229 || composingRef.current) return;
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
    // handleSubmit closes over content/captureMutation; re-bind on change so the
    // ⌘↵ submit path sees the latest draft.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen, onClose, content]);

  // Re-focus the textarea whenever the window becomes visible.
  // In standalone (Tauri window) mode, hiding the window doesn't unmount the
  // component — it just hides the webview. When it comes back via
  // `WebviewWindow.show()` the document fires `visibilitychange`; we use that
  // to restore text-entry focus so the user can type immediately.
  useEffect(() => {
    if (!isOpen) return;
    const refocus = () => {
      if (!document.hidden && editorRef.current) {
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

  // Left border accent shifts with state — mirrors MemoryCard pattern
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
        <div className="quick-capture-editor flex-1 min-h-0 overflow-y-auto pt-3">
          {fallback && <p role="status">{t("pageDetail.editor.fallbackTitle")}</p>}
          <MarkdownEditor key={session} ref={editorRef} sessionId={`quick-capture:${session}`}
            linkNotes={linkNotes} initialDocument={content} seamless minHeight={140} disabled={isPending || saved}
            ariaLabel={t("quickCapture.title")} placeholder={t("quickCapture.placeholder")}
            onDocumentChange={document => { documentRef.current = document; setContent(document); setRecoveryError(writeQuickCaptureDraft(draftKey, document) ? null : "backup"); setTooShort(false); captureMutation.reset(); }}
            onSave={handleSubmit} onCancel={() => void requestClose()} onFallback={() => setFallback(true)}
            onStatusChange={status => { composingRef.current = status.compositionActive; setComposing(status.compositionActive); }} />
        </div>

        {recoveryError && <p role="alert" className="text-xs mt-1" style={{ color: "var(--mem-status-danger-text)" }}>{t(recoveryError === "cleanup" ? "quickCapture.draftCleanupError" : "quickCapture.draftRecoveryError")}</p>}
        {(errorMessage || tooShort) && (
          <p
            role="alert"
            style={{
              fontFamily: "var(--mem-font-body)",
              fontSize: "11px",
              lineHeight: "1.4",
              color: "var(--mem-status-danger-text)",
              marginTop: 4,
              overflowWrap: "anywhere",
            }}
          >
            {tooShort
              ? t("quickCapture.tooShort")
              : t("quickCapture.saveError", { message: errorMessage })}
          </p>
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
              className="select-none"
              style={{
                fontFamily: "var(--mem-font-mono)",
                fontSize: "10px",
                color: "var(--mem-text-tertiary)",
                opacity: 0.5,
              }}
            >
              {t(standalone ? "quickCapture.standaloneShortcut" : "quickCapture.saveShortcut", { modifier: primaryShortcutModifier() })}
            </span>
            <button
              onClick={() => handleSubmit()}
              disabled={isEmpty || isPending || saved || composing}
              className="transition-all duration-200"
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
