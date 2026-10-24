// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getMemoryDetail, updateMemory, type MemoryItem } from "../../../lib/tauri";
import { confirmAction } from "../../../lib/confirmAction";
import { useReferenceNavigation } from "../links/ReferenceNavigationContext";
import { ReferencePreview } from "../links/ReferencePreview";
import type { ReferencePreviewRequest, ReferenceTarget } from "../links/referenceTypes";
import { MarkdownEditor } from "./MarkdownEditor";
import { MemoryAutosave, type MemoryAutosaveSnapshot } from "./memoryAutosave";

export function MemoryDocumentEditor({ memory, canonical, disabled, onRegisterFlush, onBusyChange }: {
  memory: MemoryItem; canonical: MemoryItem | null | undefined; disabled: boolean;
  onRegisterFlush: (flush: (() => Promise<boolean>) | null) => void;
  onBusyChange: (busy: boolean) => void;
}) {
  const { t } = useTranslation();
  const client = useQueryClient();
  const navigation = useReferenceNavigation();
  const controller = useRef<MemoryAutosave | null>(null);
  const referencePreviewLeaveTimer = useRef<number | null>(null);
  const latestEditorText = useRef(memory.content);
  const [document, setDocument] = useState({ text: memory.content, revision: 0 });
  const [snapshot, setSnapshot] = useState<MemoryAutosaveSnapshot | null>(null);
  const [referencePreview, setReferencePreview] = useState<ReferencePreviewRequest | null>(null);
  const [showLatest, setShowLatest] = useState(false);
  const disabledRef = useRef(disabled); disabledRef.current = disabled;
  const busyObserver = useRef(onBusyChange); busyObserver.current = onBusyChange;
  const initial = useRef(memory);
  useLayoutEffect(() => {
    const autosave = new MemoryAutosave({
      read: getMemoryDetail,
      write: (id, content, expectedContent) => updateMemory(id, content, undefined, undefined, undefined, expectedContent),
      onChange: next => {
        // A query can report deletion in the same batch as local input. Publish
        // draft ownership now so the parent cannot unmount it as a clean record.
        busyObserver.current(Boolean(next.dirty || next.phase === "saving"));
        setSnapshot(next);
        if (!next.dirty && next.source !== latestEditorText.current) {
          latestEditorText.current = next.source;
          setDocument(previous => ({ text: next.source, revision: previous.revision + 1 }));
        }
      },
      onCanonical: record => {
        // Cancel stale polling before replacing the content we just read back.
        void client.cancelQueries({ queryKey: ["memoryDetail", record.source_id] });
        client.setQueryData<MemoryItem>(["memoryDetail", record.source_id], previous => previous ? { ...previous, content: record.content } : previous);
        for (const queryKey of [["memory-revisions", record.source_id], ["memories"], ["knowledge-graph"], ["memoryStats"]]) {
          void client.invalidateQueries({ queryKey });
        }
      },
    });
    controller.current = autosave;
    autosave.reset(initial.current);
    onRegisterFlush(async () => !disabledRef.current && autosave.flush());
    return () => { onRegisterFlush(null); autosave.dispose(); controller.current = null; };
  }, [client, onRegisterFlush]);
  useEffect(() => { if (canonical !== undefined) controller.current?.observeCanonical(canonical); }, [canonical]);

  useEffect(() => {
    const warn = (event: BeforeUnloadEvent) => {
      if (!controller.current?.snapshot.dirty) return;
      event.preventDefault(); event.returnValue = "";
    };
    window.addEventListener("beforeunload", warn);
    return () => window.removeEventListener("beforeunload", warn);
  }, []);
  const change = useCallback((source: string) => {
    latestEditorText.current = source;
    controller.current?.setSource(source);
  }, []);
  const dismissReferencePreview = useCallback(() => {
    if (referencePreviewLeaveTimer.current !== null) window.clearTimeout(referencePreviewLeaveTimer.current);
    referencePreviewLeaveTimer.current = null;
    setReferencePreview(null);
  }, []);
  const showReferencePreview = useCallback((target: ReferenceTarget, anchor: HTMLAnchorElement, keyboard: boolean) => {
    if (referencePreviewLeaveTimer.current !== null) window.clearTimeout(referencePreviewLeaveTimer.current);
    referencePreviewLeaveTimer.current = null;
    setReferencePreview({ target, anchor, keyboard });
  }, []);
  const keepReferencePreview = useCallback(() => {
    if (referencePreviewLeaveTimer.current !== null) window.clearTimeout(referencePreviewLeaveTimer.current);
    referencePreviewLeaveTimer.current = null;
  }, []);
  const leaveReferencePreview = useCallback(() => {
    if (referencePreviewLeaveTimer.current !== null) window.clearTimeout(referencePreviewLeaveTimer.current);
    referencePreviewLeaveTimer.current = window.setTimeout(() => {
      referencePreviewLeaveTimer.current = null;
      setReferencePreview(null);
    }, 120);
  }, []);
  useEffect(() => () => {
    if (referencePreviewLeaveTimer.current !== null) window.clearTimeout(referencePreviewLeaveTimer.current);
  }, []);
  const activateReference = useCallback((target: ReferenceTarget, anchor: HTMLAnchorElement) => {
    dismissReferencePreview();
    if (target.kind === "page") navigation.onOpenPage?.(target.id);
    else if (target.kind === "memory") navigation.onOpenMemory?.(target.id);
    else if (target.citation.source_kind === "memory" && target.sourceMemory) {
      navigation.onOpenMemory?.(target.citation.locator);
    } else showReferencePreview(target, anchor, false);
  }, [dismissReferencePreview, navigation, showReferencePreview]);
  const flush = () => { if (!disabledRef.current) void controller.current?.flush(); };
  const failed = snapshot?.phase === "error";
  const conflict = snapshot?.phase === "conflict";
  return <div className="memory-document-editor page-document-editor" onBlurCapture={event => {
    if (!event.currentTarget.contains(event.relatedTarget as Node | null)) flush();
  }}>
    <MarkdownEditor initialDocument={document.text} sessionId={`${memory.source_id}:${document.revision}`} seamless
      disabled={disabled || (snapshot !== null && snapshot.baseline === null)} ariaLabel={t("memoryDetail.readingLabel")}
      onDocumentChange={change} onStatusChange={status => controller.current?.setComposing(status.compositionActive)}
      onSave={source => { change(source); flush(); }} onCancel={flush} onFallback={() => {}}
      onReferenceActivate={activateReference} onReferencePreview={showReferencePreview} />
    {snapshot && snapshot.baseline === null ? <p className="memory-save-status" role="status">{t("memoryAutosave.deleted")}</p> : null}
    {snapshot?.validation === "empty" || failed || conflict ? <div className="memory-save-status" role="alert">
      {t(snapshot?.validation === "empty" ? "memoryAutosave.empty" : conflict ? "memoryAutosave.conflict" : "memoryAutosave.failed")}
      {failed ? <button type="button" onClick={() => void controller.current?.retry()}>{t("memoryAutosave.retry")}</button> : null}
      {conflict && snapshot?.latest ? <button type="button" onClick={() => setShowLatest(value => !value)} aria-expanded={showLatest}>{t("memoryAutosave.compare")}</button> : null}
      {conflict && snapshot?.latest ? <button type="button" onClick={async () => {
        if (!await confirmAction(t("memoryAutosave.discard"), { confirm: t("memoryAutosave.loadLatest"), cancel: t("common.cancel") })) return;
        const current = controller.current?.snapshot.latest;
        if (current) { controller.current?.reset(current); setShowLatest(false); }
      }}>{t("memoryAutosave.loadLatest")}</button> : null}
      {conflict && !snapshot?.latest ? <button type="button" onClick={async () => {
        if (await confirmAction(t("memoryAutosave.discardDeleted"), { confirm: t("memoryAutosave.discardDraft"), cancel: t("common.cancel") })) controller.current?.reset(null);
      }}>{t("memoryAutosave.discardDraft")}</button> : null}
      {showLatest && conflict && snapshot?.latest ? <div className="memory-save-conflict"><strong>{t("memoryAutosave.latest")}</strong><pre>{snapshot.latest.content}</pre></div> : null}
    </div> : <span className="sr-only" role="status">{snapshot?.phase === "saving" ? t("memoryAutosave.saving") : ""}</span>}
    <ReferencePreview request={referencePreview} onDismiss={dismissReferencePreview}
      onOpenPage={navigation.onOpenPage} onOpenMemory={navigation.onOpenMemory}
      onPointerEnter={keepReferencePreview} onPointerLeave={leaveReferencePreview} />
  </div>;
}
