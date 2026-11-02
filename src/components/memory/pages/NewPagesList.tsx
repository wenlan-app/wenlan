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
 * The page's opening words for its card on Home: its first paragraph (a
 * page's short form), without Markdown marks or a heading that repeats the
 * title. Empty when the page has no prose yet.
 */
export function pagePreview(page: Pick<Page, "title" | "content" | "summary">): string {
  const body = (page.content ?? "").replace(/^---\n[\s\S]*?\n---\n?/, "");
  for (const block of body.split(/\n\s*\n/)) {
    const text = block
      .split("\n")
      .filter((line) => !/^\s*#{1,6}\s/.test(line) && !/^\s*(```|~~~|!\[)/.test(line))
      .join(" ")
      .replace(/!?\[\[([^\]|]+)(?:\|([^\]]+))?\]\]/g, (_, target, label) => label ?? target)
      .replace(/\[([^\]]+)\]\([^)]*\)/g, "$1")
      .replace(/[*_`>~]+/g, "")
      .replace(/^\s*[-+]\s+/, "")
      .replace(/\s+/g, " ")
      .trim();
    if (text && text !== page.title.trim()) return text;
  }
  return page.summary?.trim() ?? "";
}

/** A row of the home's recent list: whether the AI wrote the page, so the
 *  row can say so quietly. */
export interface RecentPage {
  readonly page: Page;
  readonly byAi: boolean;
}

/**
 * The home's recent list (UX-HOME-3): pages you edited, by when they last
 * changed, and pages the AI newly wrote, by when it wrote them. The AI's
 * small edits to its older pages don't bring them back up. A page counts as
 * yours once you wrote or edited it.
 */
export function recentPages(pages: readonly Page[], limit = NEW_PAGES_LISTED): RecentPage[] {
  return askablePages(pages)
    .map((page) => {
      const yours = page.user_edited === true || page.creation_kind === "authored";
      return { page, byAi: !yours, at: Date.parse(yours ? page.last_modified : page.created_at) || 0 };
    })
    .sort((a, b) => b.at - a.at)
    .slice(0, limit)
    .map(({ page, byAi }) => ({ page, byAi }));
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
  const rows = recentPages(pages);
  if (rows.length === 0) return null;
  return (
    <section className="wiki-new-pages" data-testid="wiki-new-pages" aria-labelledby="wiki-new-pages-title">
      <header className="wiki-new-pages-header">
        <h2 id="wiki-new-pages-title">{t("pages.overview.recentTitle")}</h2>
        {onBrowseAll ? (
          <button type="button" className="wiki-new-pages-all" onClick={onBrowseAll}>
            {t("pages.overview.seeAllPages")}
          </button>
        ) : null}
      </header>
      <ul>
        {rows.map(({ page, byAi }) => {
          const fresh = justWritten?.has(page.id) === true;
          const preview = pagePreview(page);
          return (
            <li key={page.id}>
              <button
                type="button"
                className="wiki-new-pages-row"
                data-fresh={fresh ? "true" : undefined}
                onClick={() => onSelectPage(page.id)}
              >
                <span className="wiki-new-pages-head">
                  {fresh ? (
                    <CheckCircle aria-hidden="true" weight="fill" size={16} className="wiki-new-pages-icon wiki-new-pages-icon--fresh" />
                  ) : (
                    <PageIcon className="wiki-new-pages-icon" size={16} />
                  )}
                  <span className="wiki-new-pages-title">{page.title}</span>
                </span>
                <span className="wiki-new-pages-preview">{preview}</span>
                {/* Home counts nothing, sources included (UX-HOME-2). */}
                {fresh ? <span className="wiki-new-pages-meta">{t("pages.overview.justWritten")}</span>
                  : byAi ? <span className="wiki-new-pages-by">{t("pages.overview.aiWrote")}</span> : null}
              </button>
            </li>
          );
        })}
      </ul>
    </section>
  );
}
