// SPDX-License-Identifier: AGPL-3.0-only
// "Put back the earlier text" for one change in a page's history: saves the
// text from before that change as the person's own new version, through the
// same save as typing it. The old versions stay in the history.
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageRevisions, updatePage, type PageHistoryItem } from "../../../lib/tauri";
import { Button } from "../settings/primitives";
import { historyTexts } from "./HistoryChangeDialog";

interface Props {
  readonly pageId: string;
  readonly item: PageHistoryItem;
  readonly onDone: () => void;
  /** More queries to refresh once the text is back. */
  readonly invalidate?: readonly (readonly unknown[])[];
}

export function PutBackButton({ pageId, item, onDone, invalidate = [] }: Props) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const revisions = useQuery({
    queryKey: ["page-revisions-text", pageId],
    queryFn: () => getPageRevisions(pageId, true),
    staleTime: 30_000,
    retry: false,
  });
  const before = item.kind === "source_update" || !revisions.data
    ? null
    : historyTexts(revisions.data.history ?? [], item).before;
  const revert = useMutation({
    mutationFn: async () => {
      if (before == null || !revisions.data) throw new Error("nothing to put back");
      const outcome = await updatePage({
        id: pageId,
        content: before,
        expectedVersion: revisions.data.current_version,
        callerId: "wenlan-app",
        operationId: crypto.randomUUID(),
      });
      if (outcome.outcome !== "saved") throw new Error(outcome.outcome);
    },
    onSuccess: () => {
      for (const queryKey of [["page-revisions-text", pageId], ["page-revisions", pageId], ["page", pageId], ["page-sentences", pageId], ...invalidate]) {
        void queryClient.invalidateQueries({ queryKey: [...queryKey] });
      }
      onDone();
    },
  });
  if (before == null) return null;
  return <>
    {revert.isError && <p className="space-recent-changes-error" role="alert">{t("team.changes.revertFailed")}</p>}
    <Button variant="secondary" disabled={revert.isPending} onClick={() => revert.mutate()}>{t("team.changes.revert")}</Button>
  </>;
}
