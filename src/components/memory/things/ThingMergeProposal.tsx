// SPDX-License-Identifier: AGPL-3.0-only
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { answerThingMerge, type ThingDetail } from "../../../lib/tauri";
import { THINGS_QUERY_KEY } from "../pages/WorkingOn";

/**
 * "These two look like one thing" (docs/ux/spec.md, UX-THING-3): one quiet
 * line on the thing's page, never on home. Nothing merges until the person
 * presses Merge; Keep apart means the pair is not asked about again.
 */
export function ThingMergeProposal({ thing, queryKey }: { readonly thing: ThingDetail; readonly queryKey: readonly unknown[] }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const other = (thing.merge_proposals ?? []).find((proposal) => proposal.title);
  const answer = useMutation({
    mutationFn: ({ otherId, merge }: { otherId: string; merge: boolean }) => answerThingMerge(thing.id, otherId, merge),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey });
      void queryClient.invalidateQueries({ queryKey: THINGS_QUERY_KEY });
    },
  });
  if (!other) return null;
  return (
    <div className="thing-merge" role="group" aria-label={t("thing.mergeLooksSame", { title: other.title })}>
      <p className="thing-merge-text">{t("thing.mergeLooksSame", { title: other.title })}</p>
      <span className="thing-merge-actions">
        <button type="button" className="thing-merge-yes" disabled={answer.isPending} onClick={() => answer.mutate({ otherId: other.id, merge: true })}>
          {t("thing.merge")}
        </button>
        <button type="button" className="thing-merge-no" disabled={answer.isPending} onClick={() => answer.mutate({ otherId: other.id, merge: false })}>
          {t("thing.keepApart")}
        </button>
      </span>
      {answer.isError ? <p className="thing-merge-error" role="alert">{t("thing.mergeFailed")}</p> : null}
    </div>
  );
}
