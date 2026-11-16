// SPDX-License-Identifier: AGPL-3.0-only
// "Who changed what" on a shared Space: the newest page changes, each naming
// the person or AI that made it, how many sentences it touched and when.
// Opening one shows the change and can put the page back as it was before.
import { useEffect, useState } from "react";
import { useQueries, useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import {
  getPageRevisions,
  listRecentChanges,
  type PageChange,
  type PageHistoryItem,
  type SharedStatus,
} from "../../../lib/tauri";
import { HistoryChangeDialog } from "../page/HistoryChangeDialog";
import { PutBackButton } from "../page/PutBackButton";
import { relativeMs } from "../page/format";
import { lifecycleActorName } from "../page/historyItems";
import { sharedWriter } from "../sharing/sharing";
import { writerAgentName } from "../sharing/teamPeople";
import { useTeamAccess } from "../../../web/teamAccess";
import "./space-recent-changes.css";

const SHOWN = 5;
/** The person on this computer, or on another of their own computers. */
// The same writers the core counts as a person (authorship.rs): a page started
// in the app and published, or a version the person restored, is theirs.
const PERSON = new Set(["manual_edit", "fs_edit", "human", "own_device", "person_create", "revision_accept", "publish", "restore"]);
/** Wenlan's own background organizing, not anyone's AI tool. */
const WENLAN = new Set(["distill", "re_distill", "page_growth", "refinery_merge", "citation_backfill", "link_rename"]);

/** Who made a change, said the way the row reads: a member, their AI tool,
 *  you, your AI tool, or Wenlan itself. */
export function changeWho(editedBy: string | null | undefined, shares: readonly SharedStatus[] | undefined, t: TFunction, agent?: string | null): string {
  const member = sharedWriter(editedBy, shares, t);
  if (member) return member;
  // A member's AI tool connected through the team server names its member.
  const tool = editedBy === "agent_refresh" && agent?.includes("@team.") ? writerAgentName(t, agent) : null;
  if (tool) return tool;
  if (editedBy && PERSON.has(editedBy)) return t("team.changes.you");
  if (editedBy && WENLAN.has(editedBy)) return t("team.changes.wenlan");
  return t("team.changes.yourAi");
}

interface Props {
  /** One Space, or every Space of a team (newest first across them). */
  readonly space: string | readonly string[];
  readonly shares?: readonly SharedStatus[];
}

type SpaceChange = PageChange & { readonly space: string };

export function SpaceRecentChanges({ space, shares }: Props) {
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
  if (rows.length === 0) return null;

  return (
    <section className="space-recent-changes" aria-label={t("team.changes.title")}>
      <h2 className="space-brief-heading">{t("team.changes.title")}</h2>
      <ul>
        {rows.map((change) => {
          const lifecycle = change.change_kind === "archived" || change.change_kind === "restored" ? change.change_kind : null;
          const created = !lifecycle && (change.change_kind === "created" || change.changed_sentences == null);
          let what: string;
          if (lifecycle) {
            const named = lifecycleActorName(t, change.edited_by, shares);
            what = named == null
              ? t(`team.changes.${lifecycle}Unnamed`, { title: change.title })
              : t(`team.changes.${lifecycle}`, { who: named, title: change.title });
          } else {
            const who = changeWho(change.edited_by, shares, t, change.agent);
            what = t(created ? "team.changes.added" : "team.changes.changed", { who, title: change.title });
          }
          const meta = [
            ...(several ? [change.space] : []),
            ...(lifecycle ? [] : [created ? t("team.changes.newPage") : t("team.changes.sentences", { count: change.changed_sentences ?? 0 })]),
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
  const teamAccess = useTeamAccess();
  const revisions = useQuery({
    queryKey: ["page-revisions-text", change.page_id],
    queryFn: () => getPageRevisions(change.page_id, true),
    staleTime: 30_000,
    retry: false,
  });
  const history = revisions.data?.history ?? [];
  const item: PageHistoryItem | undefined = history.find((candidate) => candidate.kind !== "source_update");

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
      // A team member puts text back only in a Space they can edit.
      action={teamAccess.member && !teamAccess.canEdit(space) ? undefined
        : <PutBackButton pageId={change.page_id} item={item} onDone={onClose} invalidate={[["recent-changes", space]]} />}
    />
  );
}
