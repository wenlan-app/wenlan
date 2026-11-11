// SPDX-License-Identifier: AGPL-3.0-only
// "Who changed what" on a shared Space: the newest page changes, each naming
// the person or AI that made it, how many sentences it touched and when.
// Opening one shows the change and can put the page back as it was before.
import { useEffect, useState, type ReactNode } from "react";
import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import {
  getPageRevisions,
  listRecentChanges,
  updatePage,
  type PageChange,
  type PageHistoryItem,
  type SharedStatus,
} from "../../../lib/tauri";
import { HistoryChangeDialog, historyTexts } from "../page/HistoryChangeDialog";
import { relativeMs } from "../page/format";
import { Button } from "../settings/primitives";
import { sharedWriter } from "../sharing/sharing";
import "./space-recent-changes.css";

const SHOWN = 5;
/** The person on this computer, or on another of their own computers. */
const PERSON = new Set(["manual_edit", "fs_edit", "human", "own_device", "person_create", "revision_accept"]);
/** Wenlan's own background organizing, not anyone's AI tool. */
const WENLAN = new Set(["distill", "re_distill", "page_growth", "refinery_merge", "citation_backfill", "link_rename"]);

/** Who made a change, said the way the row reads: a member, their AI tool,
 *  you, your AI tool, or Wenlan itself. */
export function changeWho(editedBy: string | null | undefined, shares: readonly SharedStatus[] | undefined, t: TFunction): string {
  const member = sharedWriter(editedBy, shares, t);
  if (member) return member;
  if (editedBy && PERSON.has(editedBy)) return t("team.changes.you");
  if (editedBy && WENLAN.has(editedBy)) return t("team.changes.wenlan");
  return t("team.changes.yourAi");
}

interface Props {
  /** One Space, or every Space of a team (newest first across them). */
  readonly space: string | readonly string[];
  readonly shares?: readonly SharedStatus[];
  /** Rows that wait on someone, shown above the changes. */
  readonly waiting?: ReactNode;
}

type SpaceChange = PageChange & { readonly space: string };

export function SpaceRecentChanges({ space, shares, waiting }: Props) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [open, setOpen] = useState<SpaceChange | null>(null);
  const spaces = typeof space === "string" ? [space] : space;
  const several = typeof space !== "string";
  const results = useQueries({
    queries: spaces.map((name) => ({
      queryKey: ["recent-changes", name],
      queryFn: () => listRecentChanges(SHOWN, name),
      staleTime: 15_000,
      refetchInterval: 30_000,
    })),
  });
  const rows: SpaceChange[] = results
    .flatMap((result, index) => (result.data ?? []).map((change) => ({ ...change, space: spaces[index] })))
    .sort((a, b) => b.changed_at_ms - a.changed_at_ms)
    .slice(0, SHOWN);
  if (rows.length === 0 && !waiting) return null;

  return (
    <section className="space-recent-changes" aria-label={t("team.changes.title")}>
      <h2 className="space-brief-heading">{t("team.changes.title")}</h2>
      <ul>
        {waiting}
        {rows.map((change) => {
          const who = changeWho(change.edited_by, shares, t);
          const created = change.change_kind === "created" || change.changed_sentences == null;
          const what = t(created ? "team.changes.added" : "team.changes.changed", { who, title: change.title });
          const meta = [
            ...(several ? [change.space] : []),
            created ? t("team.changes.newPage") : t("team.changes.sentences", { count: change.changed_sentences ?? 0 }),
            relativeMs(change.changed_at_ms, locale),
          ].join(" · ");
          return (
            <li key={`${change.space}:${change.page_id}`}>
              <button type="button" onClick={() => setOpen(change)}>
                <span className="space-recent-changes-what">{what}</span>
                <span className="space-recent-changes-meta">{meta}</span>
              </button>
            </li>
          );
        })}
      </ul>
      {open && <ChangeDialog change={open} space={open.space} shares={shares} onClose={() => setOpen(null)} />}
    </section>
  );
}

/** The page's newest change, with a way back to the text before it. */
function ChangeDialog({ change, space, shares, onClose }: { readonly change: PageChange; readonly space: string; readonly shares?: readonly SharedStatus[]; readonly onClose: () => void }) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const revisions = useQuery({
    queryKey: ["page-revisions-text", change.page_id],
    queryFn: () => getPageRevisions(change.page_id, true),
    staleTime: 30_000,
    retry: false,
  });
  const history = revisions.data?.history ?? [];
  const item: PageHistoryItem | undefined = history.find((candidate) => candidate.kind !== "source_update");
  const before = item ? historyTexts(history, item).before : null;
  const revert = useMutation({
    mutationFn: async () => {
      if (before == null || !revisions.data) throw new Error("nothing to put back");
      const outcome = await updatePage({
        id: change.page_id,
        content: before,
        expectedVersion: revisions.data.current_version,
        callerId: "wenlan-app",
        operationId: crypto.randomUUID(),
      });
      if (outcome.outcome !== "saved") throw new Error(outcome.outcome);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["recent-changes", space] });
      void queryClient.invalidateQueries({ queryKey: ["page-revisions-text", change.page_id] });
      onClose();
    },
  });

  const missing = !revisions.isPending && !item;
  useEffect(() => {
    if (missing) onClose();
  }, [missing, onClose]);
  if (!item) return null;
  return (
    <HistoryChangeDialog
      pageId={change.page_id}
      item={item}
      shares={shares}
      onClose={onClose}
      action={before != null && <>
        {revert.isError && <p className="space-recent-changes-error" role="alert">{t("team.changes.revertFailed")}</p>}
        <Button variant="secondary" disabled={revert.isPending} onClick={() => revert.mutate()}>{t("team.changes.revert")}</Button>
      </>}
    />
  );
}
