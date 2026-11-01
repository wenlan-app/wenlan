// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { getPage, restorePage, RestorePageApiError, type Page } from "../../../lib/tauri";
import { pageSpaceContext } from "./pagePresentation";
import { refreshPageStatus } from "./archiveWikiPage";
import { finishRenameLinks } from "./renameLinksNotice";
import "./archivedNotes.css";

/** Which message a refused restore shows. Keyed by the daemon's typed code, never its text. */
type RestoreErrorKey = "pages.archive.restoreError" | "pages.archive.titleTaken" | "pages.archive.titleTakenFixed"
  | "pages.archive.changed" | "pages.archive.fixedTitle" | "pages.archive.notRestorable";

export function restoreErrorKey(error: unknown, fixedTitle: boolean): RestoreErrorKey | null {
  if (!error) return null;
  if (!(error instanceof RestorePageApiError)) return "pages.archive.restoreError";
  switch (error.code) {
    case "page_title_taken": return fixedTitle ? "pages.archive.titleTakenFixed" : "pages.archive.titleTaken";
    case "page_version_conflict":
    case "page_not_archived": return "pages.archive.changed";
    case "page_title_fixed": return "pages.archive.fixedTitle";
    case "page_not_restorable": return "pages.archive.notRestorable";
  }
}

export function RestorePageDialog({ page, onClose, onRestored }: {
  readonly page: Page;
  readonly onClose: () => void;
  readonly onRestored?: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const id = useId();
  const [title, setTitle] = useState(page.title);
  const [snapshot, setSnapshot] = useState(page);
  const [composing, setComposing] = useState(false);
  const composingRef = useRef(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  const fixedTitle = snapshot.creation_kind === "source" || snapshot.creation_kind === "imported";
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  const setComposition = (active: boolean) => { composingRef.current = active; setComposing(active); };
  const finish = () => { onRestored?.(page.id); onClose(); };
  const restore = useMutation({
    mutationFn: () => restorePage(snapshot.id, snapshot.version, fixedTitle ? undefined : title.trim()),
    onSuccess: async result => {
      await refreshPageStatus(queryClient, page.id);
      if (result.projection_status === "pending") toast.warning(t("pages.archive.filePending"));
      else if (result.projection_status === "hidden") toast.warning(t("pages.archive.fileHidden"));
      else toast.success(t("pages.archive.restored"));
      // Under a new title the daemon also updated links to the old one; an
      // open editor on one of those pages rebases its draft from the
      // archived title, as after a rename.
      finishRenameLinks(t, queryClient, result, snapshot.title);
      finish();
    },
    onError: () => { void queryClient.invalidateQueries({ queryKey: ["pages", "archived"] }); },
  });
  const reload = useMutation({
    mutationFn: async () => {
      const latest = await getPage(page.id, "explicit");
      if (!latest || latest.id !== page.id || !["active", "archived"].includes(latest.status)) throw new Error("page is unavailable");
      return latest;
    },
    onSuccess: async latest => {
      if (latest.status === "active") {
        // Restored somewhere else in the meantime: nothing left to do here.
        await refreshPageStatus(queryClient, page.id);
        toast.success(t("pages.archive.restored"));
        finish();
      } else {
        setSnapshot(latest);
        restore.reset();
      }
    },
  });
  const busy = restore.isPending || reload.isPending;
  const error = restoreErrorKey(restore.error, fixedTitle);
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (!busy && !composingRef.current && title.trim()) restore.mutate();
  };
  return <dialog ref={dialogRef} className="wiki-restore-dialog" aria-labelledby={id}
    onCancel={event => { event.preventDefault(); if (!busy && !composingRef.current) onClose(); }}
    onKeyDownCapture={event => {
      if (event.key === "Escape" && (event.nativeEvent.isComposing || composingRef.current)) { event.preventDefault(); event.stopPropagation(); }
    }}>
    <form onSubmit={submit}>
      <h2 id={id}>{t("pages.archive.restore")}</h2>
      {pageSpaceContext(snapshot) && <p className="wiki-restore-space">{pageSpaceContext(snapshot)}</p>}
      <p>{t("pages.archive.explanation")}</p>
      <label>{t("pages.archive.titleLabel")}<input autoFocus value={title} maxLength={500} disabled={busy || fixedTitle}
        onChange={event => { setTitle(event.target.value); restore.reset(); }}
        onCompositionStart={() => setComposition(true)}
        onCompositionEnd={() => setComposition(false)}
        onBlur={() => setComposition(false)}
        onKeyDown={event => { if (event.key === "Enter" && (event.nativeEvent.isComposing || composingRef.current)) event.preventDefault(); }} /></label>
      {fixedTitle && error !== "pages.archive.fixedTitle" && <p>{t("pages.archive.fixedTitle")}</p>}
      {error && <p role="alert">{t(error)}</p>}
      {error === "pages.archive.changed" && <button type="button" className="wiki-restore-reload" disabled={busy || composing} onClick={() => reload.mutate()}>{t("pages.archive.reload")}</button>}
      {reload.isError && <p role="alert">{t("pages.archive.loadError")}</p>}
      <div className="wiki-restore-actions">
        <button type="button" onClick={onClose} disabled={busy}>{t("common.cancel")}</button>
        <button type="submit" disabled={busy || composing || !title.trim()}>{t(restore.isPending ? "pages.archive.restoring" : "pages.archive.restore")}</button>
      </div>
    </form>
  </dialog>;
}
