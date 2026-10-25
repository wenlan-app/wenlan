// SPDX-License-Identifier: AGPL-3.0-only
import { useTranslation } from "react-i18next";
import PageSearchResult from "./PageSearchResult";
import { highlightTerms } from "../../lib/highlight";
import type { EntitySearchResult, Page, SearchResult } from "../../lib/tauri";
import { passagesBeingPrepared } from "../../lib/activitySentence";
import { useActivity } from "../../lib/useActivity";

interface SearchResultsProps {
  query: string;
  memoryResults: SearchResult[];
  sourceResults: SearchResult[];
  entityResults: EntitySearchResult[];
  conceptResults: Page[];
  loading: boolean;
  error: boolean;
  ready: boolean;
  onOpenResult: (result: SearchResult) => void;
  onOpenPage: (pageId: string) => void;
  onOpenEntity: (entityId: string) => void;
}

export function SearchResults({
  query,
  memoryResults,
  sourceResults,
  entityResults,
  conceptResults,
  loading,
  error,
  ready,
  onOpenResult,
  onOpenPage,
  onOpenEntity,
}: SearchResultsProps) {
  const { t } = useTranslation();
  // The same shared query as the Activity surfaces: no extra request.
  const { data: activity } = useActivity();
  if (!ready) return null;
  const hasResults = memoryResults.length + sourceResults.length + entityResults.length + conceptResults.length > 0;
  if (!query.trim()) return null;
  // After a big import, keyword search already finds every passage while
  // search by meaning catches up in the background. One quiet line above the
  // results says so, where it is seen without scrolling.
  const preparing = passagesBeingPrepared(activity);
  const preparingLine = preparing > 0
    ? <p className="search-modal-status" data-testid="search-preparing">{t("main.search.preparing", { count: preparing })}</p>
    : null;
  if (!loading && !error && !hasResults) {
    return <>
      <p className="search-modal-status">{t("main.search.noResults", { query })}</p>
      {preparingLine}
    </>;
  }

  return (
    <div className="flex flex-col gap-2">
      {preparingLine}
      {conceptResults.length > 0 && <>
        <p className="search-results-heading">{t("main.search.pages")}</p>
        {conceptResults.map((page) => (
          <PageSearchResult key={page.id} page={page} query={query} onClick={() => onOpenPage(page.id)} />
        ))}
      </>}
      {memoryResults.length > 0 && <>
        <p className="search-results-heading" style={{ marginTop: conceptResults.length ? 12 : 0 }}>
          {t("main.search.memories", { count: memoryResults.length, query })}
        </p>
        {memoryResults.map((result) => (
          <button className="search-result-button" key={result.id} onClick={() => onOpenResult(result)} type="button">
            <span className="search-result-snippet">{highlightTerms(result.content.length > 280 ? `${result.content.slice(0, 280)}…` : result.content, query, "search-result-match")}</span>
            <span className="search-result-meta">
              {result.memory_type && <span className="search-result-kind">{result.memory_type}</span>}
              {result.source_agent && <span>{result.source_agent}</span>}
              {result.domain && <span>{result.domain}</span>}
            </span>
          </button>
        ))}
      </>}
      {sourceResults.length > 0 && <>
        <p className="search-results-heading" style={{ marginTop: memoryResults.length || conceptResults.length ? 12 : 0 }}>
          {t("main.search.sources")}
        </p>
        {sourceResults.map((result) => (
          <button className="search-result-button" key={result.id} onClick={() => onOpenResult(result)} type="button">
            <span className="search-result-snippet">{highlightTerms(result.content.length > 280 ? `${result.content.slice(0, 280)}…` : result.content, query, "search-result-match")}</span>
            <span className="search-result-meta">
              {result.source_agent && <span>{result.source_agent}</span>}
              {result.domain && <span>{result.domain}</span>}
            </span>
          </button>
        ))}
      </>}
      {entityResults.length > 0 && <>
        <p className="search-results-heading" style={{ marginTop: memoryResults.length || sourceResults.length || conceptResults.length ? 12 : 0 }}>
          {t("main.search.entities")}
        </p>
        {entityResults.map(({ entity }) => (
          <button className="search-result-button" key={entity.id} onClick={() => onOpenEntity(entity.id)} type="button">
            <span className="search-result-line">
              <span className="search-result-kind">{entity.entity_type}</span>
              <span className="search-result-title">{highlightTerms(entity.name, query, "search-result-match")}</span>
              {entity.domain && <span className="search-result-domain">{entity.domain}</span>}
            </span>
          </button>
        ))}
      </>}
    </div>
  );
}
