// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  acceptSpaceRules,
  declineSpaceRules,
  listSpaceRulesSuggestions,
  skillsFromLibraryEnabled,
  type Space,
  type SpaceRulesSuggestion,
} from "../../../lib/tauri";
import { HowIWorkLines } from "./HowIWorkLines";
import { HOW_I_WORK_QUERY_KEYS } from "./howIWorkModel";
import "./howIWork.css";

type Props = {
  readonly space: Pick<Space, "id" | "name">;
  readonly onOpenPage: (pageId: string) => void;
  /** `pending`: the same ask on a rules page or the Rules list, as a quiet
   *  box that only says the lines came from pages and do not count yet. */
  readonly variant?: "ask" | "pending";
};

export function SpaceRulesAsk({ space, onOpenPage, variant = "ask" }: Props) {
  const { t } = useTranslation();
  const queryClient = useQueryClient();
  const [hidden, setHidden] = useState(false);
  const [allSpaces, setAllSpaces] = useState(true);
  const enabled = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.enabled,
    queryFn: skillsFromLibraryEnabled,
    staleTime: 30_000,
    retry: false,
  });
  const suggestions = useQuery({
    queryKey: HOW_I_WORK_QUERY_KEYS.spaceSuggestions,
    queryFn: listSpaceRulesSuggestions,
    enabled: enabled.data === true,
    retry: false,
  });
  const suggestion = suggestions.data?.find((item) => item.space_id === space.id);

  // With the box ticked, one answer covers the whole library (UX-SKILL-1), so
  // the ask leaves every Space's page at once; cleared, only this Space's.
  const removeFromAsk = () => {
    queryClient.setQueryData<SpaceRulesSuggestion[]>(
      HOW_I_WORK_QUERY_KEYS.spaceSuggestions,
      (current) => allSpaces ? [] : current?.filter((item) => item.space_id !== space.id) ?? [],
    );
    if (allSpaces) void queryClient.invalidateQueries({ queryKey: HOW_I_WORK_QUERY_KEYS.enabled });
    setHidden(true);
  };
  const accept = useMutation({
    mutationFn: () => acceptSpaceRules(space.id, !allSpaces),
    onSuccess: (response) => {
      removeFromAsk();
      void queryClient.invalidateQueries({ queryKey: ["space-pages", space.name] });
      void queryClient.invalidateQueries({ queryKey: ["pages"] });
      if (response.rules_page_id) onOpenPage(response.rules_page_id);
    },
  });
  const decline = useMutation({
    mutationFn: () => declineSpaceRules(space.id, !allSpaces),
    onSuccess: removeFromAsk,
  });

  if (enabled.data !== true || hidden || !suggestion) return null;
  const busy = accept.isPending || decline.isPending;
  const failed = accept.isError || decline.isError;

  return (
    <section className={variant === "pending" ? "rules-pending" : "space-rules-ask"} aria-labelledby={`space-rules-ask-${space.id}`} data-testid="space-rules-ask">
      {variant === "pending" ? (
        <p className="rules-pending-label" id={`space-rules-ask-${space.id}`}>{t("howIWork.pending.label", { count: suggestion.lines.length, space: space.name })}</p>
      ) : (
        <>
          <p className="space-rules-ask-crumb">{t("howIWork.ask.crumb", { space: space.name })}</p>
          <h2 id={`space-rules-ask-${space.id}`}>{t("howIWork.ask.title", { space: space.name })}</h2>
          <p className="space-rules-ask-subtitle">{t("howIWork.ask.subtitle", { space: space.name })}</p>
        </>
      )}
      <HowIWorkLines lines={suggestion.lines} onOpenPage={onOpenPage} showEdited={false} />
      <label className="space-rules-ask-all">
        <input type="checkbox" checked={allSpaces} disabled={busy} onChange={(event) => setAllSpaces(event.target.checked)} />
        <span>{t("howIWork.ask.allSpaces")}</span>
      </label>
      <div className="how-i-work-actions">
        <button className="how-i-work-primary" disabled={busy} onClick={() => accept.mutate()} type="button">
          {t(variant === "pending" ? "howIWork.pending.accept" : "howIWork.ask.accept")}
        </button>
        <button className="how-i-work-secondary" disabled={busy} onClick={() => decline.mutate()} type="button">
          {t(variant === "pending" ? "howIWork.pending.decline" : "howIWork.ask.decline")}
        </button>
      </div>
      {failed ? <p className="how-i-work-error" role="alert">{t("howIWork.ask.failed")}</p> : null}
    </section>
  );
}
