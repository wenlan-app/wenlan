// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState, type FormEvent } from "react";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { getPage, restorePage, type Page } from "../../../lib/tauri";
import { pageSpaceContext } from "./pagePresentation";
import "./archivedNotes.css";

export function RestorePageDialog({ page, onClose }: { readonly page: Page; readonly onClose: () => void }) {
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
  const restore = useMutation({
    mutationFn: () => restorePage(snapshot.id, snapshot.version, fixedTitle ? undefined : title.trim()),
    onSuccess: async result => {
      await Promise.all([
        ["pages"], ["page", page.id], ["recent-pages"], ["searchPages"], ["search"],
        ["knowledge-graph"], ["constellation-cartography"], ["space-pages"],
        ["spaces-page-counts"], ["sidebar-space-page-counts"], ["page-links"], ["page-revisions", page.id],
      ].map(queryKey => queryClient.invalidateQueries({ queryKey })));
      if (result.projection_status === "pending") toast.warning(t("pages.archive.filePending"));
      else if (result.projection_status === "hidden") toast.warning(t("pages.archive.fileHidden"));
      else toast.success(t("pages.archive.restored"));
      onClose();
    },
    onError: () => { void queryClient.invalidateQueries({ queryKey: ["pages", "archived"] }); },
  });
  const reload = useMutation({
    mutationFn: async () => {
      const latest = await getPage(page.id);
      if (!latest || latest.id !== page.id || !["active", "archived"].includes(latest.status)) throw new Error("page is unavailable");
      return latest;
    },
    onSuccess: latest => {
      if (latest.status === "active") {
        void queryClient.invalidateQueries({ queryKey: ["pages"] });
        void queryClient.invalidateQueries({ queryKey: ["page", page.id] });
        toast.success(t("pages.archive.restored"));
        onClose();
      } else {
        setSnapshot(latest);
        restore.reset();
      }
    },
  });
  const busy = restore.isPending || reload.isPending;
  const error = restore.error ? String(restore.error).includes("page title already exists")
    ? "pages.archive.titleConflict"
    : String(restore.error).includes("page version conflict") || String(restore.error).includes("no longer archived")
      ? "pages.archive.changed" : "pages.archive.restoreError" : null;
  const submit = (event: FormEvent) => { event.preventDefault(); if (!busy && !composingRef.current && title.trim()) restore.mutate(); };
  return <dialog ref={dialogRef} className="wiki-restore-dialog" aria-labelledby={id} onCancel={event => { event.preventDefault(); if (!busy && !composingRef.current) onClose(); }} onKeyDownCapture={event => { if (event.key === "Escape" && (event.nativeEvent.isComposing || composingRef.current)) { event.preventDefault(); event.stopPropagation(); } }}>
    <form onSubmit={submit}>
      <h2 id={id}>{t("pages.archive.restore")}</h2>
      <p>{t("pages.archive.explanation")}</p>
      {pageSpaceContext(snapshot) && <p className="wiki-restore-space">{pageSpaceContext(snapshot)}</p>}
      <label>{t("pages.archive.titleLabel")}<input autoFocus value={title} maxLength={500} disabled={busy || fixedTitle}
        onChange={event => { setTitle(event.target.value); restore.reset(); }}
        onCompositionStart={() => { composingRef.current = true; setComposing(true); }}
        onCompositionEnd={() => { composingRef.current = false; setComposing(false); }}
        onKeyDown={event => { if (event.key === "Enter" && (event.nativeEvent.isComposing || composingRef.current)) event.preventDefault(); }} /></label>
      {fixedTitle && <p>{t("pages.archive.fixedTitle")}</p>}
      {error && <p role="alert">{t(error)}</p>}
      {error === "pages.archive.changed" && <button type="button" disabled={busy || composing} onClick={() => reload.mutate()}>{t("pages.archive.reload")}</button>}
      {reload.isError && <p role="alert">{t("pages.archive.loadError")}</p>}
      <div className="wiki-restore-actions"><button type="button" onClick={onClose} disabled={busy}>{t("pages.folders.cancel")}</button><button type="submit" disabled={busy || composing || !title.trim()}>{t(restore.isPending ? "pages.archive.restoring" : "pages.archive.restore")}</button></div>
    </form>
  </dialog>;
}
