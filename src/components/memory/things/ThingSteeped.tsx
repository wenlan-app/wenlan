// SPDX-License-Identifier: AGPL-3.0-only
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { undoThingDream, type ThingDetail } from "../../../lib/tauri";
import { THINGS_QUERY_KEY } from "../pages/WorkingOn";
import { steepedToday } from "./ThingViews";

/** A small crescent: the mark for what Wenlan steeped overnight. */
export function SteepMoon() {
  return (
    <svg className="steep-moon" viewBox="0 0 16 16" width="12" height="12" aria-hidden="true">
      <path d="M10.5 1.5a6.5 6.5 0 1 0 4 11.6A6 6 0 0 1 10.5 1.5z" fill="currentColor" />
    </svg>
  );
}

/**
 * What the newest night steeped from this thing into long-term memory
 * (docs/ux/spec.md, UX-CHAT-5): one quiet line under the views bar, so the
 * work done overnight is seen, and "Undo" takes that night's points back.
 */
export function ThingSteeped({ thing, queryKey }: { readonly thing: ThingDetail; readonly queryKey: readonly unknown[] }) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const undo = useMutation({
    mutationFn: () => undoThingDream(thing.id),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey });
      void queryClient.invalidateQueries({ queryKey: THINGS_QUERY_KEY });
    },
  });
  const dream = thing.dream;
  if (!dream) return null;
  const count = dream.decided + dream.open;
  const text = steepedToday(dream.at)
    ? t("thing.steepedOvernight", { count })
    : t("thing.steepedOn", {
        count,
        date: new Intl.DateTimeFormat(i18n.language, { month: "short", day: "numeric" }).format(new Date(dream.at * 1000)),
      });
  return (
    <div className="thing-steeped" role="group" aria-label={text}>
      <p className="thing-steeped-text">
        <SteepMoon />
        {text}
      </p>
      <button type="button" className="thing-steeped-undo" disabled={undo.isPending} onClick={() => undo.mutate()}>
        {t("thing.steepUndo")}
      </button>
      {undo.isError ? <p className="thing-merge-error" role="alert">{t("thing.mergeFailed")}</p> : null}
    </div>
  );
}
