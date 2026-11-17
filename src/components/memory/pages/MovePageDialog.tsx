// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { pageMove, type Page } from "../../../lib/tauri";
import { inventoryPageFilename, pageFolderPath } from "./pageInventory";
import { useKnowledgeFolders } from "./useKnowledgeFolders";
import { useTeamAccess } from "../../../web/teamAccess";
import "./wikiInventoryOverview.css";

export function MovePageDialog({ page, onClose, onMoved }: { readonly page: Page; readonly onClose: () => void; readonly onMoved?: (folderPath: string) => void }) {
  const { t } = useTranslation();
  const folders = useKnowledgeFolders();
  // A member moves a page only between its own Space's folders (each of
  // their Spaces is a top folder named after it); there is no top level.
  const member = useTeamAccess().member;
  const spaceFolder = member ? (pageFolderPath(page) ?? "").split("/")[0] : "";
  const choices = (folders.data?.folders ?? []).filter(folder => !member || (spaceFolder !== "" && (folder.path === spaceFolder || folder.path.startsWith(`${spaceFolder}/`))));
  const queryClient = useQueryClient();
  const [destination, setDestination] = useState(pageFolderPath(page) ?? "");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState(false);
  const dialogRef = useRef<HTMLDialogElement>(null);
  // Retain the receipt identity for a retry after a lost response.
  const operationRef = useRef<{ destination: string; id: string } | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);
  const move = async (event: React.FormEvent) => {
    event.preventDefault();
    const path = inventoryPageFilename(page);
    if (!path || !folders.data || folders.data.truncated || pending) return;
    setPending(true); setError(false);
    if (operationRef.current?.destination !== destination) operationRef.current = { destination, id: crypto.randomUUID() };
    try {
      const result = await pageMove(page.id, path, destination, operationRef.current.id);
      queryClient.setQueryData(["page", page.id], { ...page, storage_path: result.storage_path });
      await Promise.all([queryClient.invalidateQueries({ queryKey: ["pages"] }), queryClient.invalidateQueries({ queryKey: ["page", page.id] }), queryClient.invalidateQueries({ queryKey: ["recent-pages"] })]);
      onMoved?.(destination); onClose();
    } catch { setError(true); } finally { setPending(false); }
  };
  return <dialog ref={dialogRef} className="wiki-move-dialog" aria-labelledby="wiki-move-title" onCancel={event => { event.preventDefault(); if (!pending) onClose(); }}>
    <form onSubmit={move}><h2 id="wiki-move-title">{t("pages.folders.move")}</h2><p className="wiki-move-note">{page.title}</p>
      {folders.isPending ? <p role="status">{t("pages.overview.loading")}</p> : folders.isError || folders.data?.truncated ? <p role="status">{t("pages.folders.unavailable")}</p> : <label>{t("pages.folders.destination")}<select autoFocus value={destination} onChange={event => { setDestination(event.target.value); setError(false); }} disabled={pending}>{!member && <option value="">{t("pages.folders.rootDestination")}</option>}{choices.map(folder => <option key={folder.path} value={folder.path}>{folder.path}</option>)}</select></label>}
      {error && <p role="alert">{t("pages.folders.moveError")}</p>}
      <div className="wiki-move-actions"><button type="button" disabled={pending} onClick={onClose}>{t("pages.folders.cancel")}</button><button type="submit" disabled={pending || !folders.data || folders.data.truncated || destination === pageFolderPath(page)}>{t(pending ? "pages.folders.moving" : "pages.folders.moveConfirm")}</button></div>
    </form>
  </dialog>;
}
