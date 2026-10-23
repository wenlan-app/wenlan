// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import { CheckCircle } from "@phosphor-icons/react";
import type { Page } from "../../../lib/tauri";
import { PageIcon } from "../assets/PageIcon";
import { askablePages } from "../../onboarding/useGettingStartedState";

/** How many of the newest pages the Wiki home lists. */
export const NEW_PAGES_LISTED = 5;

/** The newest pages worth opening: written from the user's material or by
 *  the user, newest first. Copies of brought-in files are not among them. */
export function newestPages(pages: readonly Page[], limit = NEW_PAGES_LISTED): Page[] {
  return [...askablePages(pages)]
    .sort((a, b) => (Date.parse(b.created_at) || 0) - (Date.parse(a.created_at) || 0))
    .slice(0, limit);
}

/**
 * The Wiki home's way into the library: the newest pages, one click each.
 * Before this the home had no page list at all, so pages an AI tool wrote
 * from the user's material were only reachable through the directory, which
 * stays hidden while the getting-started checklist is open and opens on an
 * empty "Recently used" list. Pages the AI tool is writing right now land
 * here too, ticked, so pressing "write pages" never takes the list away.
 */
export function NewPagesList({
  pages,
  onSelectPage,
  onBrowseAll,
  justWritten,
}: {
  readonly pages: readonly Page[];
  readonly onSelectPage: (pageId: string) => void;
  readonly onBrowseAll?: () => void;
  /** Pages the current run wrote. */
  readonly justWritten?: ReadonlySet<string>;
}) {
  const { t } = useTranslation();
  const rows = newestPages(pages);
  if (rows.length === 0) return null;
  return (
    <section className="wiki-new-pages" data-testid="wiki-new-pages" aria-labelledby="wiki-new-pages-title">
      <header className="wiki-new-pages-header">
        <h2 id="wiki-new-pages-title">{t("pages.overview.newPagesTitle")}</h2>
        {onBrowseAll ? (
          <button type="button" className="wiki-new-pages-all" onClick={onBrowseAll}>
            {t("pages.overview.seeAllPages")}
          </button>
        ) : null}
      </header>
      <ul>
        {rows.map((page) => {
          const fresh = justWritten?.has(page.id) === true;
          const sources = page.source_memory_ids.length > 0
            ? t("askAi.writePages.sources", { count: page.source_memory_ids.length })
            : null;
          return (
            <li key={page.id}>
              <button
                type="button"
                className="wiki-new-pages-row"
                data-fresh={fresh ? "true" : undefined}
                onClick={() => onSelectPage(page.id)}
              >
                {fresh ? (
                  <CheckCircle aria-hidden="true" weight="fill" size={16} className="wiki-new-pages-icon wiki-new-pages-icon--fresh" />
                ) : (
                  <PageIcon className="wiki-new-pages-icon" size={16} />
                )}
                <span className="wiki-new-pages-title">{page.title}</span>
                {fresh || sources ? (
                  <span className="wiki-new-pages-meta">
                    {[fresh ? t("pages.overview.justWritten") : null, sources].filter(Boolean).join(" · ")}
                  </span>
                ) : null}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
