// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  applySkillSuggestion,
  dismissSkillSuggestion,
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
import { RulesToolsHeader } from "./RulesToolsHeader";
import {
  HOW_I_WORK_QUERY_KEYS,
  howIWorkSpaceName,
  parseHowIWorkLines,
  replaceHowIWorkLine,
  rulesPageStopped,
  setRulesPageStopped,
  visibleSourceTitle,
} from "./howIWorkModel";
import "./howIWork.css";

type Props = {
  readonly page: Page;
  readonly onOpenPage: (pageId: string) => void;
};

type SuggestionState = "applied" | "hidden";

function recordLine(record: SkillBorrowRecord, lines: readonly SkillLine[]): SkillLine | undefined {
  return lines.find((line) => line.id === record.rule_id);
}

export function HowIWorkPage({ page, onOpenPage }: Props) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const spaceName = howIWorkSpaceName(page);
  const lines = useMemo(() => parseHowIWorkLines(page.content, page), [page]);
  const stopped = rulesPageStopped(page.content);
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

  return (
    <section className="how-i-work-page" data-testid="how-i-work-page">
      <RulesToolsHeader />
      {enabled.data === true ? (
        <div className="how-i-work-page-actions" aria-label={t("howIWork.page.actionsLabel")}>
          <button className="how-i-work-quiet-action" disabled={actionBusy} onClick={() => refresh.mutate()} type="button">
            {t("howIWork.page.refresh")}
          </button>
          <button className="how-i-work-quiet-action" disabled={actionBusy || !space.data?.id} onClick={() => undoNight.mutate()} type="button">
            {t("howIWork.page.undoNight")}
          </button>
          <button
            className="how-i-work-quiet-action"
            disabled={actionBusy}
            onClick={() => { void saveContent(setRulesPageStopped(page.content, !stopped)); }}
            type="button"
          >
            {t(stopped ? "howIWork.page.start" : "howIWork.page.stop")}
          </button>
        </div>
      ) : null}

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

      <HowIWorkLines lines={lines} onOpenPage={onOpenPage} onEditLine={editLine} />
      <p className="how-i-work-stop-note">{t(stopped ? "howIWork.page.stoppedNote" : "howIWork.page.stopNote")}</p>

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
