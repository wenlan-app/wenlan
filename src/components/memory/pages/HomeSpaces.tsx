// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { listSpaces, setUiLanguage, type Page, type Space } from "../../../lib/tauri";

/** The lead Space plus two rows of three under it. */
const SPACES_LISTED = 7;

/** The Space used last leads: the one holding the page changed most
 *  recently. Under it come the starred Spaces, then the rest; within each,
 *  the one used last first, and Spaces with no pages in the person's own
 *  order, as in the sidebar. */
export function homeSpaceOrder(spaces: readonly Space[], pages: readonly Page[] = []): Space[] {
  const lastUsed = new Map<string, number>();
  for (const page of pages) {
    if (!page.space) continue;
    const at = Date.parse(page.last_modified || page.created_at) || 0;
    if (at > (lastUsed.get(page.space) ?? 0)) lastUsed.set(page.space, at);
  }
  const used = (space: Space) => lastUsed.get(space.name) ?? 0;
  const byRecent = (a: Space, b: Space) => used(b) - used(a) || a.sort_order - b.sort_order;
  const own = spaces.filter((space) => !space.suggested);
  const lead = own.slice().sort((a, b) => used(b) - used(a) || Number(b.starred) - Number(a.starred) || a.sort_order - b.sort_order)[0];
  if (!lead) return [];
  const rest = own.filter((space) => space !== lead)
    .sort((a, b) => Number(b.starred) - Number(a.starred) || byRecent(a, b));
  return [lead, ...rest];
}

/**
 * Home's first section while "What you're working on" is an experiment that
 * is off (UX-HOME-1): the person's Spaces. The one used last leads,
 * set large, so opening the app says where to carry on; starred ones, then
 * the rest, sit under it in three columns. The night still sorts conversations and steeps them in the
 * background; only the cards for that work are left off this screen.
 */
export function HomeSpaces({
  onSelectSpace,
  onBrowseSpaces,
  pages,
}: {
  readonly pages?: readonly Page[];
  readonly onSelectSpace: (spaceName: string) => void;
  readonly onBrowseSpaces?: () => void;
}) {
  const { t, i18n } = useTranslation();
  const spaces = useQuery({ queryKey: ["spaces"], queryFn: listSpaces });
  // The background still names what it reads in the app's language.
  const language = i18n.resolvedLanguage ?? i18n.language;
  useEffect(() => {
    if (language) void setUiLanguage(language).catch(() => {});
  }, [language]);
  const ordered = homeSpaceOrder(spaces.data ?? [], pages);
  const listed = ordered.slice(0, SPACES_LISTED);
  if (listed.length === 0) return null;
  const [lead, ...rest] = listed;
  return (
    <section className="home-spaces" data-testid="home-spaces" aria-labelledby="home-spaces-title">
      <header className="wiki-new-pages-header home-spaces-header">
        <h2 id="home-spaces-title">{t("pages.overview.spacesTitle")}</h2>
        {ordered.length > listed.length && onBrowseSpaces ? (
          <button type="button" className="wiki-new-pages-all" onClick={onBrowseSpaces}>
            {t("pages.overview.spacesAll")}
          </button>
        ) : null}
      </header>
      <button type="button" className="home-space-card home-space-lead" onClick={() => onSelectSpace(lead.name)}>
        <span className="home-space-lead-line">
          <span className="home-space-title">{lead.name}</span>
          <span className="home-space-lead-tag">{t("pages.overview.spacesLastUsed")}</span>
        </span>
        {lead.description ? <span className="home-space-text">{lead.description}</span> : null}
      </button>
      {rest.length > 0 ? (
        <ul>
          {rest.map((space) => (
            <li key={space.id}>
              <button type="button" className="home-space-card" onClick={() => onSelectSpace(space.name)}>
                <span className="home-space-title" title={space.name}>{space.name}</span>
                {space.description ? <span className="home-space-text" title={space.description}>{space.description}</span> : null}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
    </section>
  );
}
