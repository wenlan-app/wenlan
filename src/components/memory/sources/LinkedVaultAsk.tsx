// SPDX-License-Identifier: AGPL-3.0-only
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listLinkedVaults, setLinkedVaultAiEdits, type LinkedVault } from "../../../lib/tauri";
import "../page/NoteReviewNotice.css";

export const LINKED_VAULTS_QUERY_KEY = ["linked-vaults"] as const;

/** A vault linked before today that the person has not yet been asked about. */
export function vaultToAsk(vaults: readonly LinkedVault[], now: Date = new Date()): LinkedVault | null {
  return vaults.find((vault) => {
    if (vault.ai_edits !== undefined && vault.ai_edits !== null) return false;
    if (vault.linked_at === undefined || vault.linked_at === null) return false;
    return new Date(vault.linked_at * 1000).toDateString() !== now.toDateString();
  }) ?? null;
}

/**
 * Home, from the day after a vault is linked: asks once whether the AI may
 * change its notes directly. Until the person says yes the notes' pages stay
 * locked, so an AI change waits for them; no answer counts as no. Nothing at
 * all when there is nothing to ask, or when linked vaults cannot be read.
 */
export function LinkedVaultAsk() {
  const { t } = useTranslation();
  const qc = useQueryClient();
  const vaults = useQuery({ queryKey: LINKED_VAULTS_QUERY_KEY, queryFn: listLinkedVaults, retry: false });
  const answer = useMutation({
    mutationFn: ({ path, allow }: { path: string; allow: boolean }) => setLinkedVaultAiEdits(path, allow),
    onSuccess: (data) => qc.setQueryData(LINKED_VAULTS_QUERY_KEY, data),
  });
  const vault = vaultToAsk(vaults.data?.vaults ?? []);
  if (!vault) return null;
  return <div className="note-review-notice linked-vault-ask" role="group" aria-label={t("sourceAccess.askTitle", { name: vault.folder })}>
    <div className="note-review-notice__content">
      <span className="note-review-notice__summary">{t("sourceAccess.askTitle", { name: vault.folder })}</span>
      <p className="note-review-notice__message">{t("sourceAccess.askBody")}</p>
    </div>
    <div className="note-review-notice__actions">
      <button type="button" className="note-review-notice__button note-review-notice__button--quiet" disabled={answer.isPending}
        onClick={() => answer.mutate({ path: vault.path, allow: false })}>
        {t("sourceAccess.askNo")}
      </button>
      <button type="button" className="note-review-notice__button" disabled={answer.isPending}
        onClick={() => answer.mutate({ path: vault.path, allow: true })}>
        {t("sourceAccess.askYes")}
      </button>
    </div>
  </div>;
}
