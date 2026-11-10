// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { detectObsidianVaults, linkObsidianVault } from "../../../lib/tauri";
import SourceDialog from "./SourceDialog";
import { KNOWLEDGE_FOLDERS_QUERY_KEY } from "../pages/useKnowledgeFolders";

/** The daemon's refusal codes this dialog says in words. */
const REFUSALS = {
  linked_vault_is_a_source: "sourceAccess.linkIsSource",
  linked_vault_already_linked: "sourceAccess.linkTaken",
  linked_vault_overlaps_pages_folder: "sourceAccess.linkOverlaps",
} as const;

function folderName(path: string): string {
  return path.replace(/\\/g, "/").replace(/\/+$/, "").split("/").pop() ?? path;
}

/**
 * Link an Obsidian vault into the Wiki (UX-ADD-3): its notes become pages
 * the person edits directly, kept the same as the vault's files. Nothing
 * of Wenlan's goes into the vault.
 */
export default function LinkVaultDialog({ onClose, initialPath }: {
  onClose: () => void;
  /** A vault the person already picked as a folder, chosen here at once. */
  initialPath?: string;
}) {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const { data: found } = useQuery({ queryKey: ["obsidian-vaults"], queryFn: detectObsidianVaults });
  const [picked, setPicked] = useState<string | null>(initialPath ?? null);
  const [failure, setFailure] = useState<string | null>(null);

  const browse = async () => {
    const path = await openDialog({ title: t("sourceAccess.linkTitle"), directory: true, multiple: false });
    if (typeof path === "string") {
      setFailure(null);
      setPicked(path);
    }
  };

  const link = useMutation({
    onMutate: () => setFailure(null),
    mutationFn: (path: string) => linkObsidianVault(path),
    onSuccess: (_, path) => {
      void qc.invalidateQueries({ queryKey: KNOWLEDGE_FOLDERS_QUERY_KEY });
      void qc.invalidateQueries({ queryKey: ["pages"] });
      toast(t("sourceAccess.linkDone", { name: folderName(path) }), { description: t("sourceAccess.linkDoneBody") });
      onClose();
    },
    onError: (error) => {
      const message = error instanceof Error ? error.message : String(error);
      const code = (Object.keys(REFUSALS) as (keyof typeof REFUSALS)[]).find((key) => message.includes(key));
      setFailure(code ? t(REFUSALS[code]) : message);
    },
  });

  const vaults = found ?? [];
  const browsedOther = picked !== null && !vaults.some((vault) => vault.path === picked);
  return (
    <SourceDialog title={t("sourceAccess.linkTitle")} onClose={onClose}>
      <div className="link-vault">
        <span className="link-vault-label">{t("sourceAccess.linkFound")}</span>
        <div className="link-vault-choices">
          {vaults.map((vault) => (
            <button
              key={vault.path}
              type="button"
              className="link-vault-choice"
              aria-pressed={picked === vault.path}
              title={vault.path}
              onClick={() => { setFailure(null); setPicked(vault.path); }}
            >
              {vault.name}
            </button>
          ))}
          {browsedOther && (
            <button type="button" className="link-vault-choice" aria-pressed="true" title={picked ?? undefined}>
              {folderName(picked ?? "")}
            </button>
          )}
          <button type="button" className="link-vault-choice link-vault-other" onClick={() => void browse()}>
            {t("sourceAccess.linkOther")}
          </button>
        </div>
        <div className="link-vault-lines">
          <p>{t("sourceAccess.linkLine1")}</p>
          <p>{t("sourceAccess.linkLine2")}</p>
        </div>
        {failure && <div role="alert" className="source-dialog-error">{failure}</div>}
        <div className="link-vault-actions">
          <button type="button" className="link-vault-cancel" onClick={onClose}>{t("sourceAccess.linkCancel")}</button>
          <button
            type="button"
            className="link-vault-submit"
            disabled={picked === null || link.isPending}
            onClick={() => picked && link.mutate(picked)}
          >
            {t("sourceAccess.linkSubmit")}
          </button>
        </div>
      </div>
    </SourceDialog>
  );
}
