// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { PageIcon } from "../assets/PageIcon";
import type { Page, SharedStatus } from "../../../lib/tauri";
import { relativeMs } from "../page/format";
import { sharedWriter } from "../sharing/sharing";
import "../sharing/sharing.css";
import type { AssetLens } from "../../../lib/assetLens";
import type { SpaceDetailCopy } from "./copy";
import {
  PAGE_DISPLAY_STEP,
  PAGE_FETCH_LIMIT,
  pageCountLabel,
  sortedSpacePages,
} from "./model";

type SpaceDossierNavigation = {
  readonly onSelectPage: (pageId: string) => void;
};

type SpaceDossierContentProps = {
  readonly copy: SpaceDetailCopy;
  readonly navigation: SpaceDossierNavigation;
  readonly pages: readonly Page[];
  readonly lens: AssetLens;
  /** Everything shared: a row says who changed the page when it was not
   *  this person. */
  readonly shares?: readonly SharedStatus[];
};

export function SpaceDossierContent({ copy, navigation, pages, lens, shares }: SpaceDossierContentProps) {
  const { t, i18n } = useTranslation();
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const [visiblePageCount, setVisiblePageCount] = useState(PAGE_DISPLAY_STEP);
  const sortedPages = useMemo(() => sortedSpacePages(pages), [pages]);
  const visiblePages = sortedPages.slice(0, visiblePageCount);

  return (
    <div className="space-dossier-content">
      <section aria-label={copy.metrics.pages} className="space-dossier-pages">
        {sortedPages.length === 0 ? (
          <p className="space-dossier-empty">{copy.noPages}</p>
        ) : (
          <div className="space-dossier-page-list" data-lens={lens}>
            {visiblePages.map((page) => (
              <button key={page.id} onClick={() => navigation.onSelectPage(page.id)} type="button">
                <PageIcon />
                <span className="space-dossier-page-text">
                  <span className="space-dossier-page-title">{page.title}</span>{" "}
                  {page.summary?.trim() && <span className="space-dossier-page-summary">{page.summary}</span>}
                </span>
                <SharedRowMeta page={page} shares={shares} locale={locale} />
              </button>
            ))}
          </div>
        )}
        {visiblePageCount < sortedPages.length && (
          <button className="space-dossier-text-action" onClick={() => setVisiblePageCount((current) => current + PAGE_DISPLAY_STEP)} type="button">
            {t("spaceDetail.showMore")}
          </button>
        )}
        {pages.length >= PAGE_FETCH_LIMIT && (
          <p className="space-dossier-empty" role="status">
            {t("spaceDetail.pageLimit", { count: pageCountLabel(pages.length, locale), limit: PAGE_FETCH_LIMIT.toLocaleString(locale) })}
          </p>
        )}
      </section>

    </div>
  );
}

/** "Yiling · 2h ago" when another member changed the page last; nothing for
 *  this person's own changes. */
function SharedRowMeta({ page, shares, locale }: { readonly page: Page; readonly shares?: readonly SharedStatus[]; readonly locale: string }) {
  const { t } = useTranslation();
  const who = sharedWriter(page.last_edited_by, shares, t);
  if (!who) return null;
  const at = Date.parse(page.last_modified);
  return <span className="shared-row-meta">{t("sharing.rowBy", { who, when: Number.isFinite(at) ? relativeMs(at, locale) : "" })}</span>;
}
