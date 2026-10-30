// SPDX-License-Identifier: AGPL-3.0-only
import { useId, useMemo, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import type { Page } from "../../../lib/tauri";
import { ARCHIVED_PAGES_QUERY_KEY, listAllArchivedPagesExplicitBrowse, isKnowledgePage, EXPLICIT_BROWSE_QUERY_POLICY } from "./listAllPages";
import { RestorePageDialog } from "./RestorePageDialog";
import { pageSpaceContext } from "./pagePresentation";
import "./archivedNotes.css";

const archivedLabel = (page: Page) => [page.title, pageSpaceContext(page)].filter(Boolean).join(" · ");

/** The Archived pages browse mode: find an archived page, open it read only, or restore it. */
export function ArchivedNotesPanel({ currentPageId, onOpenPage, onRestored }: {
  readonly currentPageId?: string | null;
  readonly onOpenPage: (page: Page) => void;
  readonly onRestored?: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  const [filter, setFilter] = useState("");
  const searchId = useId();
  const [restoring, setRestoring] = useState<Page | null>(null);
  const query = useQuery({ queryKey: ARCHIVED_PAGES_QUERY_KEY, queryFn: listAllArchivedPagesExplicitBrowse, ...EXPLICIT_BROWSE_QUERY_POLICY });
  const pages = useMemo(() => (query.data ?? []).filter(isKnowledgePage), [query.data]);
  const matches = useMemo(() => {
    const needle = filter.trim().toLocaleLowerCase();
    return pages.filter(page => archivedLabel(page).toLocaleLowerCase().includes(needle));
  }, [pages, filter]);
  return <section className="wiki-archived-notes" aria-label={t("pages.archive.title")}>
    <label className="sr-only" htmlFor={searchId}>{t("pages.archive.search")}</label>
    <input id={searchId} type="search" value={filter} onChange={event => setFilter(event.target.value)} placeholder={t("pages.archive.search")} />
    {query.isPending ? <p role="status">{t("pages.overview.loading")}</p>
      : query.isError ? <div role="alert"><p>{t("pages.archive.loadError")}</p><button type="button" onClick={() => void query.refetch()}>{t("pages.archive.retry")}</button></div>
      : matches.length === 0 ? <p>{t(pages.length ? "pages.archive.noMatches" : "pages.archive.empty")}</p>
      : <ul>{matches.map(page => <li key={page.id} className="wiki-archived-row" data-active={page.id === currentPageId ? "true" : undefined}>
        <button className="wiki-archived-open" type="button" onClick={() => onOpenPage(page)} title={archivedLabel(page)} aria-label={archivedLabel(page)}>
          <span>{page.title}</span>{pageSpaceContext(page) && <small>{pageSpaceContext(page)}</small>}
        </button>
        <button type="button" className="wiki-archived-restore" aria-label={t("pages.archive.restoreNamed", { title: archivedLabel(page) })} onClick={() => setRestoring(page)}>{t("pages.archive.restoreShort")}</button>
      </li>)}</ul>}
    {restoring && <RestorePageDialog key={restoring.id} page={restoring} onClose={() => setRestoring(null)} onRestored={onRestored} />}
  </section>;
}
