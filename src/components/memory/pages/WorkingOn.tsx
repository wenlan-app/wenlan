// SPDX-License-Identifier: AGPL-3.0-only
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listWorkingOn, type WorkingOnItem } from "../../../lib/tauri";

export const WORKING_ON_QUERY_KEY = ["working-on"] as const;

/** How many lines of work home shows; the rest live in their Spaces. */
export const WORKING_ON_LISTED = 6;

/** "Today", "yesterday", "3 days ago" in the interface language: when the
 *  work was added, a day and never a counter of things. */
export function addedWhen(language: string, addedAtSeconds: number, now = Date.now()): string {
  const days = Math.max(0, Math.floor((now - addedAtSeconds * 1000) / 86_400_000));
  const format = new Intl.RelativeTimeFormat(language, { numeric: "auto" });
  return days < 7 ? format.format(-days, "day") : format.format(-Math.floor(days / 7), "week");
}

/**
 * Home's first section: what you're working on (docs/ux/spec.md, UX-HOME-1).
 * Until things have a record of their own, each active item of a Space Brief
 * stands for one thing; its text says where that work stands, and opening it
 * goes to the Space it lives in. Nothing shows when no Space has active work,
 * and the section never counts anything (UX-HOME-2).
 */
export function WorkingOn({ onSelectSpace }: { readonly onSelectSpace: (spaceName: string) => void }) {
  const { t, i18n } = useTranslation();
  const { data } = useQuery({
    queryKey: WORKING_ON_QUERY_KEY,
    queryFn: listWorkingOn,
    refetchInterval: 60_000,
  });
  const items: WorkingOnItem[] = (data ?? []).slice(0, WORKING_ON_LISTED);
  if (items.length === 0) return null;
  return (
    <section className="wiki-working-on" data-testid="wiki-working-on" aria-labelledby="wiki-working-on-title">
      <h2 id="wiki-working-on-title">{t("pages.overview.workingOnTitle")}</h2>
      <ul>
        {items.map((item) => (
          <li key={`${item.space}\u0000${item.id}`}>
            <button type="button" className="wiki-working-on-card" onClick={() => onSelectSpace(item.space)}>
              <span className="wiki-working-on-text">{item.text}</span>
              <span className="wiki-working-on-meta">
                {item.space} · {addedWhen(i18n.language, item.added_at)}
              </span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}
