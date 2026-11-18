// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo, useState } from "react";
import { createPortal } from "react-dom";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowClockwise, ArrowCounterClockwise, Play, Stop } from "@phosphor-icons/react";
import { useTranslation } from "react-i18next";
import {
  applySkillSuggestion,
  dismissSkillSuggestion,
  getPageRevisions,
  getSpace,
  listSkillBorrowRecords,
  listSkillSuggestions,
  refreshSkills,
  skillsFromLibraryEnabled,
  undoSkillSuggestion,
  undoSpaceRules,
  updatePage,
  type Page,
  type SkillBorrowRecord,
  type SkillLine,
  type SkillSuggestion,
} from "../../../lib/tauri";
import { HowIWorkLines } from "./HowIWorkLines";
import { OverflowMenu } from "../page/OverflowMenu";
import { formatTimeAgo } from "../../../lib/dateFormat";
import { RulesToolsHeader, usePageRules } from "./RulesToolsHeader";
import {
  HOW_I_WORK_QUERY_KEYS,
  howIWorkSpaceName,
  lastRulesWrite,
  maintainedSourceCount,
  pageGivesGeneralRules,
  parseHowIWorkLines,
  replaceHowIWorkLine,
  rulesPageStopped,
  setRulesPageStopped,
  visibleSourceTitle,
  withDaemonAuthors,
} from "./howIWorkModel";
import "./howIWork.css";

type Props = {
  readonly page: Page;
  readonly onOpenPage: (pageId: string) => void;
  /** The place at the right end of the page's title row where the "…" menu
   *  goes. Undefined: the page has no title row of its own here, and the
   *  menu heads the page. Null: the row is not mounted yet. */
  readonly titleActionsHost?: HTMLElement | null;
};

/** When the page's rules were last written, and what says so: the page's
 *  history when it is loaded, else the page's own modified time. */
export type RulesUpdatedAt = { readonly at: number; readonly from: "history" | "modified" } | null;

export function rulesUpdatedAt(page: Pick<Page, "last_modified">, history: readonly { at: number; edited_by: string }[] | undefined): RulesUpdatedAt {
  if (history) {
    const at = lastRulesWrite(history);
    return at === null ? null : { at, from: "history" };
  }
  const modified = Date.parse(page.last_modified) / 1000;
  return Number.isFinite(modified) ? { at: Math.floor(modified), from: "modified" } : null;
}

/** One quiet line under the title: "You edited · AI updated it from N
 *  sources · 2h ago", each part only when it is true, and nothing at all
 *  when none is. The time belongs to the AI's update and shows only with
 *  it. */
function HowIWorkStatus({ page, lines }: { readonly page: Page; readonly lines: readonly SkillLine[] }) {
  const { t, i18n } = useTranslation();
  const history = useQuery({
    queryKey: ["page-revisions", page.id],
    queryFn: () => getPageRevisions(page.id),
    staleTime: 30_000,
    retry: false,
  });
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const youEdited = page.user_edited === true || Boolean(page.human_edited_at);
  const sources = maintainedSourceCount(lines);
  const updated = sources > 0 ? rulesUpdatedAt(page, history.data?.entries) : null;
  const parts: { key: string; text: string; from?: string }[] = [];
  if (youEdited) parts.push({ key: "you", text: t("howIWork.page.status.youEdited") });
  if (sources > 0) parts.push({ key: "ai", text: t("howIWork.page.status.aiUpdated", { count: sources }) });
  if (updated) parts.push({ key: "when", text: locale.startsWith("en") ? formatTimeAgo(updated.at) : formatTimeAgo(updated.at, locale), from: updated.from });
  if (parts.length === 0) return null;
  return (
    <p className="how-i-work-status" data-testid="how-i-work-status">
      {parts.map((part, index) => (
        <span key={part.key} data-part={part.key} data-from={part.from}>
          {index > 0 ? <span className="how-i-work-status-sep" aria-hidden="true">·</span> : null}
          {part.key === "when" ? <time dateTime={new Date(updated!.at * 1000).toISOString()}>{part.text}</time> : part.text}
        </span>
      ))}
    </p>
  );
}

type SuggestionState = "applied" | "hidden";

function recordLine(record: SkillBorrowRecord, lines: readonly SkillLine[]): SkillLine | undefined {
  return lines.find((line) => line.id === record.rule_id);
}

export function HowIWorkPage({ page, onOpenPage, titleActionsHost }: Props) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const spaceName = howIWorkSpaceName(page);
  // Who wrote a bullet, and which rules are used in every Space, are the
  // daemon's answers; the page text only says which lines there are.
  const rules = usePageRules(page, spaceName || null);
  const lines = useMemo(() => withDaemonAuthors(parseHowIWorkLines(page.content, page), rules, page.id), [page, rules]);
  const stopped = rulesPageStopped(page.content);
  const delivers = !stopped && pageGivesGeneralRules(rules, page, lines, !spaceName);
  const [writeBusy, setWriteBusy] = useState(false);
  const [writeError, setWriteError] = useState(false);
  const [suggestionStates, setSuggestionStates] = useState<Record<string, SuggestionState>>({});

  const enabled = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.enabled,
    queryFn: skillsFromLibraryEnabled,
    staleTime: 30_000,
    retry: false,
  });
  const space = useQuery({
    queryKey: ["space", spaceName],
    queryFn: () => getSpace(spaceName),
    enabled: enabled.data === true && Boolean(spaceName),
    retry: false,
  });
  const records = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.records(spaceName),
    queryFn: () => listSkillBorrowRecords(spaceName),
    enabled: enabled.data === true && !stopped && Boolean(spaceName),
    retry: false,
  });
  const suggestions = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.skillSuggestions,
    queryFn: listSkillSuggestions,
    enabled: enabled.data === true && !stopped,
    retry: false,
  });

  const refresh = useMutation({
    mutationFn: refreshSkills,
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["page", page.id] });
      void queryClient.invalidateQueries({ queryKey: ["pages"] });
      void queryClient.invalidateQueries({ queryKey: HOW_I_WORK_QUERY_KEYS.skillSuggestions });
      void queryClient.invalidateQueries({ queryKey: HOW_I_WORK_QUERY_KEYS.records(spaceName) });
    },
  });
  const undoNight = useMutation({
    mutationFn: async () => {
      if (!space.data?.id) throw new Error("Space unavailable");
      return undoSpaceRules(space.data.id);
    },
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["page", page.id] });
      void queryClient.invalidateQueries({ queryKey: ["pages"] });
    },
  });
  const suggestionAction = useMutation({
    mutationFn: async ({ suggestion, action }: { suggestion: SkillSuggestion; action: "apply" | "dismiss" | "undo" }) => {
      if (action === "apply") return applySkillSuggestion(suggestion.id);
      if (action === "dismiss") return dismissSkillSuggestion(suggestion.id);
      return undoSkillSuggestion(suggestion.id);
    },
    onSuccess: (_response, { suggestion, action }) => {
      setSuggestionStates((current) => ({
        ...current,
        [suggestion.id]: action === "apply" ? "applied" : "hidden",
      }));
    },
  });

  const saveContent = async (nextContent: string): Promise<boolean> => {
    if (writeBusy || nextContent === page.content) return nextContent === page.content;
    setWriteBusy(true);
    setWriteError(false);
    try {
      const outcome = await updatePage({
        id: page.id,
        content: nextContent,
        expectedVersion: page.version,
        callerId: "wenlan-app",
        operationId: globalThis.crypto.randomUUID(),
      });
      if (outcome.outcome !== "saved") throw new Error(outcome.outcome);
      await queryClient.invalidateQueries({ queryKey: ["page", page.id] });
      void queryClient.invalidateQueries({ queryKey: ["pages"] });
      return true;
    } catch {
      setWriteError(true);
      return false;
    } finally {
      setWriteBusy(false);
    }
  };

  const editLine = (line: SkillLine, text: string) => {
    const content = replaceHowIWorkLine(page.content, line, text);
    if (!content) {
      setWriteError(true);
      return Promise.resolve(false);
    }
    return saveContent(content);
  };

  const recentRecords = [...(records.data ?? [])].sort((a, b) => b.at - a.at).slice(0, 3);
  const matchingSuggestions = (suggestions.data ?? []).filter((suggestion) =>
    lines.some((line) => line.id === suggestion.source_rule_id)
      && suggestionStates[suggestion.id] !== "hidden",
  );
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const actionFailed = refresh.isError || undoNight.isError || suggestionAction.isError || writeError;
  const actionBusy = refresh.isPending || undoNight.isPending || writeBusy;

  // Update now, Undo last night and Stop live behind the "…" at the end of
  // the title row; the Stop sentence sits under Stop (owner's 2026-11-18
  // approval of sketch rules-page-v2).
  const menu = enabled.data === true ? (
    <OverflowMenu
      className="how-i-work-menu"
      label={t("howIWork.page.actionsLabel")}
      items={[
        { key: "refresh", label: t("howIWork.page.refresh"), icon: <ArrowClockwise size={16} />, disabled: actionBusy, onSelect: () => refresh.mutate() },
        { key: "undo", label: t("howIWork.page.undoNight"), icon: <ArrowCounterClockwise size={16} />, disabled: actionBusy || !space.data?.id, onSelect: () => undoNight.mutate() },
        {
          key: "stop",
          label: t(stopped ? "howIWork.page.start" : "howIWork.page.stop"),
          hint: t(stopped ? "howIWork.page.stoppedNote" : "howIWork.page.stopNote"),
          icon: stopped ? <Play size={16} /> : <Stop size={16} />,
          disabled: actionBusy,
          onSelect: () => { void saveContent(setRulesPageStopped(page.content, !stopped)); },
        },
      ]}
    />
  ) : null;

  return (
    <section className="how-i-work-page" data-testid="how-i-work-page">
      {titleActionsHost === undefined ? <div className="how-i-work-title-actions how-i-work-title-actions--inline">{menu}</div> : null}
      {titleActionsHost ? createPortal(menu, titleActionsHost) : null}
      <div className="how-i-work-head">
        <HowIWorkStatus page={page} lines={lines} />
        <RulesToolsHeader delivers={delivers} />
      </div>
      {recentRecords.length > 0 ? (
        <section className="how-i-work-borrowed" aria-labelledby="how-i-work-borrowed-title">
          <h2 id="how-i-work-borrowed-title">{t("howIWork.page.borrowedTitle")}</h2>
          <ul>
            {recentRecords.map((record, index) => {
              const line = recordLine(record, lines);
              const day = new Intl.DateTimeFormat(locale, { weekday: "short" }).format(new Date(record.at * 1000));
              return (
                <li key={`${record.at}:${record.rule_id}:${index}`}>
                  {record.self_report === "changed_plan"
                    ? t("howIWork.page.borrowedChanged", { day, rule: line?.text ?? visibleSourceTitle(record.page_title, t) })
                    : t("howIWork.page.borrowedRead", { day, page: visibleSourceTitle(record.page_title, t) })}
                  {" · "}<span className="how-i-work-borrowed-label">{t(record.self_report === "changed_plan" ? "howIWork.page.reportedByTool" : "howIWork.page.recordedByWenlan")}</span>
                </li>
              );
            })}
          </ul>
        </section>
      ) : null}

      <HowIWorkLines lines={lines} onOpenPage={onOpenPage} onEditLine={editLine} ownPageId={page.id} />

      {matchingSuggestions.length > 0 ? (
        <section className="how-i-work-suggestions" aria-label={t("howIWork.suggestion.sectionTitle")}>
          {matchingSuggestions.map((suggestion) => {
            const source = lines.find((line) => line.id === suggestion.source_rule_id);
            const sourceTitle = visibleSourceTitle(suggestion.source_page, t);
            const sourceDate = source?.source_revision_at
              ? new Intl.DateTimeFormat(locale, { month: "short", day: "numeric" }).format(new Date(source.source_revision_at * 1000))
              : "";
            const applied = suggestionStates[suggestion.id] === "applied";
            const busy = suggestionAction.isPending && suggestionAction.variables?.suggestion.id === suggestion.id;
            return (
              <article className="how-i-work-suggestion" key={suggestion.id}>
                <p className="how-i-work-suggestion-title">{t("howIWork.suggestion.title", { skill: suggestion.skill })}</p>
                <div className="how-i-work-suggestion-diff">
                  {suggestion.old_line ? <span className="how-i-work-suggestion-old">{suggestion.old_line}</span> : null}
                  <span className="how-i-work-suggestion-new">{suggestion.new_line}</span>
                </div>
                <p className="how-i-work-suggestion-source">
                  {t("howIWork.suggestion.basedOn")}{" "}
                  {source ? <button type="button" onClick={() => onOpenPage(source.source_page_id)}>{sourceTitle}</button> : sourceTitle}
                  {sourceDate ? ` (${sourceDate})` : ""}
                </p>
                <div className="how-i-work-suggestion-actions">
                  {applied ? (
                    <button className="how-i-work-secondary" disabled={busy} onClick={() => suggestionAction.mutate({ suggestion, action: "undo" })} type="button">
                      {t("howIWork.suggestion.undo")}
                    </button>
                  ) : (
                    <>
                      <button className="how-i-work-primary" disabled={busy} onClick={() => suggestionAction.mutate({ suggestion, action: "apply" })} type="button">
                        {t("howIWork.suggestion.apply")}
                      </button>
                      <button className="how-i-work-secondary" disabled={busy} onClick={() => suggestionAction.mutate({ suggestion, action: "dismiss" })} type="button">
                        {t("howIWork.suggestion.dismiss")}
                      </button>
                    </>
                  )}
                </div>
              </article>
            );
          })}
        </section>
      ) : null}

      {actionFailed ? <p className="how-i-work-error" role="alert">{t("howIWork.page.actionFailed")}</p> : null}
    </section>
  );
}
