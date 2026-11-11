// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getSpaceBrief } from "../../../lib/tauri";
import { relativeMs } from "../page/format";
import "./space-brief.css";

/** Items shown before "N more". */
export const BRIEF_ITEMS_SHOWN = 3;

/**
 * A Space's Brief on top of its page (docs/ux/spec.md, UX-SPACE-1): where
 * the work stands, when an AI tool last handed off, and what is in progress,
 * in plain lines the night writes (UX-SPACE-2). Nothing shows while it loads,
 * fails, or the Space has no Brief: the page is then its pages alone.
 */
export function SpaceBrief({ spaceName }: { readonly spaceName: string }) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [open, setOpen] = useState(false);
  const query = useQuery({
    queryKey: ["space-brief", spaceName],
    queryFn: () => getSpaceBrief(spaceName),
    retry: false,
  });
  const brief = query.data;
  if (!brief) return null;
  const shown = open ? brief.active : brief.active.slice(0, BRIEF_ITEMS_SHOWN);
  const hidden = brief.active.length - BRIEF_ITEMS_SHOWN;
  return (
    <section className="space-brief" aria-label={t("spaceDetail.brief.label")} data-testid="space-brief" data-split={Boolean(brief.summary) && brief.active.length > 0}>
      {brief.summary ? (
        <div className="space-brief-now">
          <p className="space-brief-summary">{brief.summary}</p>
          {brief.handed_off_at ? (
            <p className="space-brief-meta">{t("spaceDetail.brief.handedOff", { when: relativeMs(brief.handed_off_at * 1000, locale) })}</p>
          ) : null}
        </div>
      ) : null}
      {brief.active.length > 0 ? (
        <div className="space-brief-items">
          <h2 className="space-brief-heading">{t("spaceDetail.brief.inProgress")}</h2>
          <ul>
            {shown.map((item) => <li key={item.id}>{item.line}</li>)}
          </ul>
          {hidden > 0 ? (
            <button type="button" className="space-brief-more" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
              {open ? t("spaceDetail.brief.less") : t("spaceDetail.brief.more", { count: hidden })}
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d="m6 9 6 6 6-6" /></svg>
            </button>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}
