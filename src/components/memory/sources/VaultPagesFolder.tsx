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

/** A home-relative path reads shorter: `~/.wenlan/pages`. */
export function shortPath(path: string): string {
  const inner = normalized(path);
  const home = /^(\/Users\/[^/]+|\/home\/[^/]+|[A-Za-z]:\/Users\/[^/]+)(?=\/|$)/.exec(inner);
  return home ? `~${inner.slice(home[0].length)}` : inner;
}

/** A path that wraps only after a slash, never inside a folder name. */
function PathText({ path }: { path: string }) {
  const parts = path.split("/");
  return <span className="vault-pages-path">
    {parts.map((part, index) => <span key={index}>
      <span className="vault-pages-path-part">{part}{index < parts.length - 1 ? "/" : ""}</span>
      {index < parts.length - 1 && <wbr />}
    </span>)}
  </span>;
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
  const [message, setMessage] = useState<{ key: CopyKey; alert?: boolean } | null>(null);
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
      await queryClient.invalidateQueries({ queryKey: ["knowledgePath"] });
      const now = await queryClient.fetchQuery({ queryKey: ["knowledgePathInfo"], queryFn: getKnowledgePathInfo, staleTime: 0 });
      // A move waiting for a restart by hand shows as waiting, below.
      if (outcome === "byHand") { setMessage(null); return; }
      setMessage(normalized(now.path) === normalized(folder)
        ? { key: "sourceContainer.pagesMoved" }
        : { key: "sourceContainer.pagesMoveFailed", alert: true });
    },
    onError: (error) => {
      console.warn("[sources] pages folder move failed", error);
      setTarget(null);
      setMessage({ key: refusalKey(error), alert: true });
    },
  });

  if (!info.data) return null;
  const pending = info.data.pending_path;
  const current = pending ?? info.data.path;
  const shown = (path: string) => shownFromVault(path, source.path) ?? shortPath(path);
  const inside = shownFromVault(current, source.path) !== null;

  const pick = async () => {
    setMessage(null);
    const chosen = await openDialog({ directory: true, multiple: false, defaultPath: source.path });
    if (typeof chosen === "string") setTarget(chosen);
  };
  const proposed = `${normalized(source.path)}/Wenlan`;
  // After a refusal the suggested folder is the problem: let them pick.
  const refused = message?.alert === true;
  const idle = target === null && !move.isPending;

  return (
    <section className="vault-pages-folder" aria-label={t("sourceContainer.pagesHere")}>
      <h3 className="note-info-section-heading">{t("sourceContainer.pagesHere")}</h3>
      {target === null
        ? <p className="vault-pages-folder-path"><PathText path={shown(current)} /></p>
        : <div className="vault-pages-folder-confirm">
          <p>{t("sourceContainer.pagesConfirm")}</p>
          <p className="vault-pages-folder-path"><PathText path={shown(target)} /></p>
          <p>{t("sourceContainer.pagesConfirmHint")}</p>
          {!move.isPending && <div className="vault-pages-folder-actions">
            <button type="button" className="collection-control vault-pages-folder-primary" onClick={() => move.mutate(target)}>
              {t("sourceContainer.pagesConfirmMove")}
            </button>
            <button type="button" className="collection-control" onClick={() => setTarget(null)}>
              {t("sourceContainer.pagesCancel")}
            </button>
          </div>}
        </div>}
      {move.isPending && <p role="status">{t("sourceContainer.pagesMoving")}</p>}
      {message && <p role={message.alert ? "alert" : "status"}>{t(message.key)}</p>}
      {!message && idle && (
        pending
          ? <p role="status">{t("sourceContainer.pagesRestartToFinish")}</p>
          : info.data.move_error
            ? <p role="alert">{t("sourceContainer.pagesMoveFailed")}</p>
            : <p>{t(inside ? "sourceContainer.pagesHereHint" : "sourceContainer.pagesElsewhereHint")}</p>
      )}
      {idle && !pending && (
        <button type="button" className="collection-control vault-pages-folder-button" onClick={inside || refused ? () => void pick() : () => { setMessage(null); setTarget(proposed); }}>
          {t(inside || refused ? "sourceContainer.pagesChange" : "sourceContainer.pagesMoveHere")}
        </button>
      )}
    </section>
  );
}
