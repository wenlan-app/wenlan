// SPDX-License-Identifier: AGPL-3.0-only
// Joining a Space or a page someone shared: choose the folder they shared,
// see what it holds, and add it.
import { useEffect, useRef, useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  createSpace,
  listSpaces,
  lookInSharedFolder,
  sharePage,
  shareSpace,
  type SharedFolder,
} from "../../../lib/tauri";
import { FolderIcon } from "./ShareDialog";
import { SHARES_KEY, shortFolder, useShares } from "./sharing";
import "./sharing.css";

type JoinSharedDialogProps = {
  readonly onClose: () => void;
  /** Called once it is added: the Space's name here, or the page's id. */
  readonly onJoined?: (kind: "space" | "page", target: string) => void;
};

function namesOf(folder: SharedFolder): string {
  return folder.members.map((member) => member.name).join(", ");
}

export function JoinSharedDialog({ onClose, onJoined }: JoinSharedDialogProps) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const shares = useShares();
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  const dialogRef = useRef<HTMLDialogElement>(null);
  const [folder, setFolder] = useState<string | null>(null);
  const [found, setFound] = useState<SharedFolder | null | undefined>(undefined);
  const [isLibrary, setIsLibrary] = useState(false);
  const [localName, setLocalName] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const dialog = dialogRef.current;
    if (dialog && !dialog.open) dialog.showModal();
    return () => dialog?.close();
  }, []);

  const choose = async () => {
    const chosen = await openDialog({ directory: true, multiple: false });
    if (typeof chosen !== "string") return;
    setFolder(chosen);
    setError(null);
    setFound(undefined);
    setIsLibrary(false);
    try {
      const shared = await lookInSharedFolder(chosen);
      // A library is joined from Settings, not here.
      setIsLibrary(shared?.kind === "library");
      setFound(shared?.kind === "library" ? null : shared);
      if (shared) setLocalName(shared.name);
    } catch (caught) {
      setFound(null);
      setError(caught instanceof Error ? caught.message : String(caught));
    }
  };

  const spaceNames = new Set((spaces.data ?? []).map((space) => space.name.trim().toLocaleLowerCase()));
  const nameTaken = found?.kind === "space" && spaceNames.has(found.name.trim().toLocaleLowerCase());
  const chosenName = nameTaken ? localName.trim() : found?.name ?? "";
  const stillTaken = nameTaken && spaceNames.has(chosenName.toLocaleLowerCase());
  const already = found && (shares.data ?? []).some((share) => share.folder === folder
    || (found.kind === "page" && share.kind === "page" && share.target === found.page_id));

  const join = async (event: React.FormEvent) => {
    event.preventDefault();
    if (!folder || !found || already || pending || (found.kind === "space" && (!chosenName || stillTaken))) return;
    setPending(true);
    setError(null);
    try {
      if (found.kind === "space") {
        await createSpace(chosenName);
        await shareSpace(chosenName, folder);
      } else if (found.page_id) {
        await sharePage(found.page_id, folder);
      }
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
        queryClient.invalidateQueries({ queryKey: ["spaces"] }),
        queryClient.invalidateQueries({ queryKey: ["pages"] }),
        queryClient.invalidateQueries({ queryKey: ["recent-pages"] }),
      ]);
      onClose();
      onJoined?.(found.kind === "space" ? "space" : "page", found.kind === "space" ? chosenName : found.page_id ?? "");
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : String(caught));
    } finally {
      setPending(false);
    }
  };

  return (
    <dialog
      ref={dialogRef}
      className="share-dialog"
      aria-labelledby="join-dialog-title"
      onCancel={(event) => { event.preventDefault(); if (!pending) onClose(); }}
    >
      <form onSubmit={join}>
        <h2 id="join-dialog-title">{t("sharing.joinTitle")}</h2>
        <div className="share-field">
          <span className="share-label">{t("sharing.folder")}</span>
          <div className="share-folder-row">
            <span className="share-folder" data-empty={folder === null}>
              <FolderIcon />
              <span>{folder ? shortFolder(folder, t) : t("sharing.noFolder")}</span>
            </span>
            <button type="button" className="share-secondary" onClick={() => void choose()} disabled={pending}>{t("sharing.choose")}</button>
          </div>
          <p className="share-hint">{t("sharing.joinFolderHint")}</p>
          {folder === null && <p className="share-hint">{t("sharing.joinFindHint")}</p>}
        </div>
        {folder !== null && found === null && !error && <p className="share-warning">{t(isLibrary ? "sharing.joinIsLibrary" : "sharing.joinNotShared")}</p>}
        {found && (
          <div className="share-found">
            <strong>{found.name}</strong>
            <span>{t(found.kind === "space" ? "sharing.joinKindSpace" : "sharing.joinKindPage", { names: namesOf(found) })}</span>
          </div>
        )}
        {found?.kind === "page" && !already && <p className="share-hint">{t("sharing.joinPageLands")}</p>}
        {nameTaken && !already && (
          <label className="share-field">
            <span className="share-label">{t("sharing.joinNameTaken")}</span>
            <input className="share-input" value={localName} onChange={(event) => setLocalName(event.target.value)} disabled={pending} />
          </label>
        )}
        {already && <p className="share-warning">{t("sharing.joinAlready")}</p>}
        {error && <p className="share-warning" role="alert">{t("sharing.joinFailed", { message: error })}</p>}
        <div className="share-actions">
          <button type="button" className="share-secondary" onClick={onClose} disabled={pending}>{t("sharing.cancel")}</button>
          <button
            type="submit"
            className="share-primary"
            disabled={!found || Boolean(already) || pending || (found.kind === "space" && (!chosenName || stillTaken))}
          >
            {t(pending ? "sharing.joining" : "sharing.join")}
          </button>
        </div>
      </form>
    </dialog>
  );
}
