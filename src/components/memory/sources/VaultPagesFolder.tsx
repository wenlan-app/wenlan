// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useTranslation } from "react-i18next";
import {
  getKnowledgePathInfo,
  restartDaemonToMovePages,
  setKnowledgePath,
  type RegisteredSource,
} from "../../../lib/tauri";
import type { enSourceContainer } from "./sourceContainerCopy";

type CopyKey = `sourceContainer.${keyof typeof enSourceContainer}`;

function normalized(path: string) { return path.replace(/\\/g, "/").replace(/\/+$/, ""); }
function basename(path: string) { return normalized(path).split("/").pop() || path; }

/** `path` as the person sees it from inside the vault: `My notes/Wenlan`. */
export function shownFromVault(path: string, vault: string): string | null {
  const root = normalized(vault);
  const inner = normalized(path);
  if (!inner.startsWith(`${root}/`)) return null;
  return `${basename(root)}${inner.slice(root.length)}`;
}

/** The copy key for a refused move, from the daemon's validation code. */
export function refusalKey(error: unknown): CopyKey {
  const text = String(error);
  if (text.includes("pages_folder_not_empty")) return "sourceContainer.pagesNotEmpty";
  if (text.includes("pages_folder_nested") || text.includes("pages_folder_unchanged")) return "sourceContainer.pagesNested";
  if (text.includes("pages_folder_in_app_folder")) return "sourceContainer.pagesAppFolder";
  return "sourceContainer.pagesBadFolder";
}

const RESTART_BY_HAND = new Set(["daemon-restart:isolated", "daemon-restart:not-owned"]);

/**
 * Where Wenlan's own pages live, shown in an Obsidian vault's info, with a
 * way to move them into the vault so they open in Obsidian next to the
 * person's notes. The move runs when the background service restarts.
 */
export function VaultPagesFolder({ source }: { source: RegisteredSource }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [target, setTarget] = useState<string | null>(null);
  const [message, setMessage] = useState<{ key: CopyKey; folder?: string; alert?: boolean } | null>(null);
  const info = useQuery({ queryKey: ["knowledgePathInfo"], queryFn: getKnowledgePathInfo });

  const move = useMutation({
    mutationFn: async (folder: string) => {
      await setKnowledgePath(folder);
      try {
        await restartDaemonToMovePages();
      } catch (error) {
        if (RESTART_BY_HAND.has(String(error))) return "byHand" as const;
        throw error;
      }
      return "restarted" as const;
    },
    onSuccess: async (outcome, folder) => {
      setTarget(null);
      await queryClient.invalidateQueries({ queryKey: ["knowledgePathInfo"] });
      await queryClient.invalidateQueries({ queryKey: ["knowledgePath"] });
      if (outcome === "byHand") {
        setMessage({ key: "sourceContainer.pagesRestartToFinish" });
        return;
      }
      const now = await queryClient.fetchQuery({ queryKey: ["knowledgePathInfo"], queryFn: getKnowledgePathInfo });
      const shown = shownFromVault(folder, source.path) ?? folder;
      setMessage(normalized(now.path) === normalized(folder)
        ? { key: "sourceContainer.pagesMoved", folder: shown }
        : { key: "sourceContainer.pagesMoveFailed", folder: now.path, alert: true });
    },
    onError: (error) => {
      console.warn("[sources] pages folder move failed", error);
      setTarget(null);
      setMessage({ key: refusalKey(error), alert: true });
    },
  });

  if (!info.data) return null;
  const current = info.data.path;
  const inside = shownFromVault(current, source.path);
  const pending = info.data.pending_path;

  const pick = async () => {
    setMessage(null);
    const chosen = await openDialog({ directory: true, multiple: false, defaultPath: source.path });
    if (typeof chosen === "string") setTarget(chosen);
  };
  const proposed = `${normalized(source.path)}/Wenlan`;
  // After a refusal the suggested folder is the problem: let them pick.
  const refused = message?.alert === true;

  return (
    <section className="vault-pages-folder" aria-label={t("sourceContainer.pagesHere")}>
      <h3 className="note-info-section-heading">{t("sourceContainer.pagesHere")}</h3>
      <div className="vault-pages-folder-row">
        <span className="source-container-path">{inside ?? current}</span>
        {target === null && !move.isPending && (
          <button type="button" className="collection-control" onClick={inside || refused ? () => void pick() : () => { setMessage(null); setTarget(proposed); }}>
            {t(inside || refused ? "sourceContainer.pagesChange" : "sourceContainer.pagesMoveHere")}
          </button>
        )}
      </div>
      {target !== null && !move.isPending && (
        <div className="vault-pages-folder-confirm">
          <p>{t("sourceContainer.pagesConfirm", { folder: shownFromVault(target, source.path) ?? target })}</p>
          <div className="vault-pages-folder-actions">
            <button type="button" className="collection-control vault-pages-folder-primary" onClick={() => move.mutate(target)}>
              {t("sourceContainer.pagesConfirmMove")}
            </button>
            <button type="button" className="collection-control" onClick={() => setTarget(null)}>
              {t("sourceContainer.pagesCancel")}
            </button>
          </div>
        </div>
      )}
      {move.isPending && <p role="status">{t("sourceContainer.pagesMoving")}</p>}
      {message && <p role={message.alert ? "alert" : "status"}>{message.folder === undefined
        ? t(message.key)
        // Every message that names a folder takes it as `{{folder}}`.
        : t(message.key as "sourceContainer.pagesMoved", { folder: message.folder })}</p>}
      {!message && !move.isPending && target === null && (
        pending
          ? <p>{t("sourceContainer.pagesPending", { folder: shownFromVault(pending, source.path) ?? pending })}</p>
          : info.data.move_error
            ? <p role="alert">{t("sourceContainer.pagesMoveFailed", { folder: inside ?? current })}</p>
            : <p>{t(inside ? "sourceContainer.pagesHereHint" : "sourceContainer.pagesElsewhereHint")}</p>
      )}
    </section>
  );
}
