// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import PageSearchResult from "./PageSearchResult";
import { highlightTerms } from "../../lib/highlight";
import {
  agentDisplayName,
  getConversationLines,
  type ConversationHit,
  type ConversationLine,
  type EntitySearchResult,
  type Page,
  type SearchResult,
  type ThingSummary,
} from "../../lib/tauri";
import { passagesBeingPrepared } from "../../lib/activitySentence";
import { useActivity } from "../../lib/useActivity";
import { resolveAgentDisplayName } from "../../lib/agents";
import { relativeTime } from "../../lib/relativeTime";

interface SearchResultsProps {
  query: string;
  /** What you're working on, matched by title or where it stands. */
  thingResults: ThingSummary[];
  conceptResults: Page[];
  /** Sentences from AI conversations (UX-FIND-1). */
  conversationHits: ConversationHit[];
  /**
   * Conversations are still being read. The other groups show meanwhile;
   * "nothing found" waits for them.
   */
  conversationsLoading?: boolean;
  /** Passages from source files, shown with the conversation sentences. */
  sourceResults: SearchResult[];
  entityResults: EntitySearchResult[];
  loading: boolean;
  error: boolean;
  ready: boolean;
  onOpenThing: (thingId: string) => void;
  onOpenResult: (result: SearchResult) => void;
  onOpenPage: (pageId: string) => void;
  onOpenEntity: (entityId: string) => void;
}

/** What a line says, with Markdown links read as their text. */
export function readable(text: string): string {
  return text.replace(/\[([^\]]+)\]\([^)\s]+\)/g, "$1");
}

/** Every whitespace-separated word of `query` is in `text`, ignoring case. */
export function matchesAllWords(text: string, query: string): boolean {
  const words = query.toLowerCase().split(/\s+/).filter(Boolean);
  const lower = text.toLowerCase();
  return words.length > 0 && words.every((word) => lower.includes(word));
}

/** Things whose title or status line has every search word, newest first. */
export function matchingThings(things: readonly ThingSummary[], query: string, limit = 3): ThingSummary[] {
  return things
    .filter((thing) => matchesAllWords(`${thing.title ?? ""}\n${thing.status_line ?? ""}`, query))
    .slice(0, limit);
}

/**
 * One search box for everything (UX-FIND-1). Groups, in order: what you're
 * working on, pages, points on a thing's map (once maps are written), the
 * words said in conversations, then topics.
 */
export function SearchResults({
  query,
  thingResults,
  conceptResults,
  conversationHits,
  conversationsLoading = false,
  sourceResults,
  entityResults,
  loading,
  error,
  ready,
  onOpenThing,
  onOpenResult,
  onOpenPage,
  onOpenEntity,
}: SearchResultsProps) {
  const { t, i18n } = useTranslation();
  // The same shared query as the Activity surfaces: no extra request.
  const { data: activity } = useActivity();
  const [openHit, setOpenHit] = useState<ConversationHit | null>(null);
  useEffect(() => setOpenHit(null), [query]);
  if (!ready || !query.trim()) return null;
  if (openHit) {
    return <ConversationQuote
      hit={openHit}
      query={query}
      onBack={() => setOpenHit(null)}
      onOpenThing={onOpenThing}
    />;
  }
  const saidCount = conversationHits.length + sourceResults.length;
  const hasResults = thingResults.length + conceptResults.length + saidCount + entityResults.length > 0;
  // After a big import, keyword search already finds every passage while
  // search by meaning catches up in the background. One quiet line above the
  // results says so, where it is seen without scrolling.
  const preparing = passagesBeingPrepared(activity);
  const preparingLine = preparing > 0
    ? <p className="search-modal-status" data-testid="search-preparing">{t("main.search.preparing", { count: preparing })}</p>
    : null;
  if (!loading && !error && !hasResults) {
    if (conversationsLoading) return preparingLine;
    return <>
      <p className="search-modal-status">{t("main.search.noResults", { query })}</p>
      {preparingLine}
    </>;
  }
  const when = (at: number | null | undefined) => (at ? relativeTime(at, t, i18n.language) : null);

  return (
    <div className="search-results-groups">
      {preparingLine}
      {thingResults.length > 0 && <section className="search-results-group" aria-label={t("main.search.things")}>
        <p className="search-results-heading">{t("main.search.things")}</p>
        {thingResults.map((thing) => (
          <button className="search-result-button" key={thing.id} onClick={() => onOpenThing(thing.id)} type="button">
            <span className="search-result-title">{highlightTerms(thing.title ?? t("main.search.untitledThing"), query, "search-result-match")}</span>
            {thing.status_line && <span className="search-result-summary search-result-summary--flush">{highlightTerms(thing.status_line, query, "search-result-match")}</span>}
            <span className="search-result-meta"><span>{when(thing.last_activity_at)}</span></span>
          </button>
        ))}
      </section>}
      {conceptResults.length > 0 && <section className="search-results-group" aria-label={t("main.search.pages")}>
        <p className="search-results-heading">{t("main.search.pages")}</p>
        {conceptResults.map((page) => (
          <PageSearchResult key={page.id} page={page} query={query} onClick={() => onOpenPage(page.id)} />
        ))}
      </section>}
      {saidCount > 0 && <section className="search-results-group" aria-label={t("main.search.said")}>
        <p className="search-results-heading">{t("main.search.said")}</p>
        {conversationHits.map((hit) => (
          <button
            className="search-result-button"
            key={`${hit.session_id}:${hit.index}`}
            onClick={() => setOpenHit(hit)}
            type="button"
          >
            <span className="search-result-snippet">{highlightTerms(readable(hit.line.text), query, "search-result-match")}</span>
            <span className="search-result-meta">
              <span>{speakerName(hit.line, hit.tool, t("main.search.you"))}</span>
              {hit.line.speaker === "you" && <span>{agentDisplayName(hit.tool)}</span>}
              {hit.line.at ? <span>{when(hit.line.at)}</span> : null}
              {(hit.thing_title ?? hit.title) && <span>{hit.thing_title ?? hit.title}</span>}
            </span>
          </button>
        ))}
        {sourceResults.map((result) => (
          <button className="search-result-button" key={result.id} onClick={() => onOpenResult(result)} type="button">
            <span className="search-result-snippet">{highlightTerms(result.content.length > 280 ? `${result.content.slice(0, 280)}…` : result.content, query, "search-result-match")}</span>
            <span className="search-result-meta">
              {result.title && <span>{result.title}</span>}
              {result.source_agent && <span>{resolveAgentDisplayName(result.source_agent)}</span>}
            </span>
          </button>
        ))}
      </section>}
      {entityResults.length > 0 && <section className="search-results-group" aria-label={t("main.search.entities")}>
        <p className="search-results-heading">{t("main.search.entities")}</p>
        {entityResults.map(({ entity }) => (
          <button className="search-result-button" key={entity.id} onClick={() => onOpenEntity(entity.id)} type="button">
            <span className="search-result-line">
              <span className="search-result-kind">{entity.entity_type}</span>
              <span className="search-result-title">{highlightTerms(entity.name, query, "search-result-match")}</span>
              {entity.domain && <span className="search-result-domain">{entity.domain}</span>}
            </span>
          </button>
        ))}
      </section>}
    </div>
  );
}

function speakerName(line: ConversationLine, tool: string, you: string): string {
  return line.speaker === "you" ? you : agentDisplayName(tool) ?? tool;
}

/**
 * The sentence with a little of what came before and after, and where it is
 * used; "show all" opens the whole conversation at that line (UX-FIND-2).
 */
function ConversationQuote({ hit, query, onBack, onOpenThing }: {
  readonly hit: ConversationHit;
  readonly query: string;
  readonly onBack: () => void;
  readonly onOpenThing: (thingId: string) => void;
}) {
  const { t } = useTranslation();
  const [all, setAll] = useState(false);
  const lines = useQuery({
    queryKey: ["conversationLines", hit.session_id],
    queryFn: () => getConversationLines(hit.session_id),
    enabled: all,
  });
  const focusRef = useRef<HTMLParagraphElement>(null);
  const backRef = useRef<HTMLButtonElement>(null);
  useEffect(() => { backRef.current?.focus(); }, []);
  useEffect(() => {
    if (all && lines.data) focusRef.current?.scrollIntoView?.({ block: "center" });
  }, [all, lines.data]);
  const said = (line: ConversationLine, focus: boolean, key: string | number) => (
    <p
      className={focus ? "search-quote-line search-quote-line--hit" : "search-quote-line"}
      key={key}
      ref={focus ? focusRef : undefined}
    >
      <span className="search-quote-speaker">{speakerName(line, hit.tool, t("main.search.you"))}</span>
      <span className="search-quote-text">{focus ? highlightTerms(readable(line.text), query, "search-result-match") : readable(line.text)}</span>
    </p>
  );
  return (
    <div className="search-quote" data-testid="conversation-quote">
      <button className="search-quote-back" onClick={onBack} ref={backRef} type="button">{t("main.search.backToResults")}</button>
      <div className="search-quote-lines">
        {all && lines.data
          ? lines.data.lines.map((line, i) => {
            const at = i + (lines.data.first_index ?? 0);
            return said(line, at === hit.index, at);
          })
          : <>
            {hit.before && said(hit.before, false, "before")}
            {said(hit.line, true, "hit")}
            {hit.after && said(hit.after, false, "after")}
          </>}
        {all && lines.isError && <p className="search-modal-status">{t("main.search.conversationGone")}</p>}
      </div>
      {hit.thing_id && (
        <p className="search-quote-used">
          {t("main.search.usedIn")}{" "}
          <button className="search-quote-link" onClick={() => onOpenThing(hit.thing_id!)} type="button">
            {hit.thing_title ?? hit.title ?? t("main.search.untitledThing")}
          </button>
        </p>
      )}
      {!all && (
        <button className="search-quote-link" onClick={() => setAll(true)} type="button">{t("main.search.showAll")}</button>
      )}
    </div>
  );
}
