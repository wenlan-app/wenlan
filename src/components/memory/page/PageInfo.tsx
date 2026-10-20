// SPDX-License-Identifier: AGPL-3.0-only
import { useState } from "react";
import { PageIcon } from "../assets/PageIcon";
import { useTranslation } from "react-i18next";
import type {
  BasisRef,
  MemoryItem,
  PageChangelogEntry,
  PageCitation,
  PageLinkInbound,
  PageSourceWithMemory,
} from "../../../lib/tauri";
import type { CitationState } from "../../../lib/pageCitations";
import { basisCountLabel, prettyAgent, relativeMs, revisionSummary, revisionWriterLabel } from "./format";
import PageInfoNotes from "./PageInfoNotes";
import "./NoteInfo.css";

interface PageInfoProps {
  /** Render inside PageDetail's single expandable information section. */
  embedded?: boolean;
  sourceCount: number;
  sources: PageSourceWithMemory[] | undefined;
  inbound: PageLinkInbound[];
  revisions: PageChangelogEntry[];
  revisionsLoading?: boolean;
  revisionsError?: boolean;
  onRetryRevisions?: () => void;
  citations: PageCitation[] | undefined;
  citationState: CitationState;
  onMemoryClick: (sourceId: string) => void;
  onPageClick?: (pageId: string) => void;
  /** Notes this note relies on. With the handlers below, the group can be edited. */
  currentPageId?: string;
  currentSpace?: string | null;
  basis?: readonly BasisRef[];
  onMarkBasis?: (pageId: string) => Promise<boolean>;
  onUnmarkBasis?: (pageId: string) => void;
}

// Content chips already carry per-claim provenance; inside this panel the
// long lists stay capped so the unique sections read without scrolling.
const SOURCES_SHOWN = 5;
const REVISIONS_SHOWN = 3;

const showAllStyle = {
  fontFamily: "var(--mem-font-mono)",
  fontSize: "10px",
  color: "var(--mem-text-secondary)",
  cursor: "pointer",
  padding: "4px 2px",
  background: "none",
  border: "none",
  textAlign: "left",
} as const;

const groupHeading = {
  fontFamily: "var(--mem-font-mono)",
  fontSize: "10px",
  fontWeight: 600,
  letterSpacing: "0.05em",
  textTransform: "uppercase",
  color: "var(--mem-text-tertiary)",
} as const;

// Known memory_type values get a localized label; anything else is data and
// passes through untouched.
const KNOWN_SOURCE_KINDS = new Set(["memory", "chat", "file", "obsidian", "web"]);

/** Cited rows first (by first occurrence), then uncited by recency. */
function sortSourceRows(
  rows: PageSourceWithMemory[],
  citations: PageCitation[] | undefined,
): PageSourceWithMemory[] {
  const firstOccurrence = new Map<string, number>();
  for (const c of [...(citations ?? [])].sort((a, b) => a.occurrence - b.occurrence)) {
    if (c.source_kind === "memory" && !firstOccurrence.has(c.locator)) {
      firstOccurrence.set(c.locator, c.occurrence);
    }
  }
  return [...rows].sort((a, b) => {
    const ao = firstOccurrence.get(a.source.memory_source_id);
    const bo = firstOccurrence.get(b.source.memory_source_id);
    if (ao != null && bo != null) return ao - bo;
    if (ao != null) return -1;
    if (bo != null) return 1;
    return (b.memory?.last_modified ?? 0) - (a.memory?.last_modified ?? 0);
  });
}

export default function PageInfo({
  embedded = false,
  sourceCount,
  sources,
  inbound,
  revisions,
  revisionsLoading = false,
  revisionsError = false,
  onRetryRevisions,
  citations,
  citationState,
  onMemoryClick,
  onPageClick,
  currentPageId,
  currentSpace = null,
  basis,
  onMarkBasis,
  onUnmarkBasis,
}: PageInfoProps) {
  const { t, i18n } = useTranslation();
  // English keeps the legacy abbreviated relatives ("1d ago"); Chinese gets
  // a real locale so "1d ago" does not leak into the zh UI.
  const relativeLocale = i18n.language?.startsWith("zh") ? i18n.language : undefined;
  const rows = sortSourceRows(
    (sources ?? []).filter((s) => s.memory !== null),
    citations,
  );
  const [showAllSources, setShowAllSources] = useState(false);
  const [showAllRevisions, setShowAllRevisions] = useState(false);
  const visibleRows = showAllSources ? rows : rows.slice(0, SOURCES_SHOWN);
  // Daemon order varies; newest revision is what the panel is opened for.
  const revisionsDesc = [...revisions].sort((a, b) => b.version - a.version);
  const visibleRevisions = showAllRevisions
    ? revisionsDesc
    : revisionsDesc.slice(0, REVISIONS_SHOWN);
  const seenInboundIds = new Set<string>();
  const uniqueInbound = inbound.filter((link) => {
    if (seenInboundIds.has(link.source_page_id)) return false;
    seenInboundIds.add(link.source_page_id);
    return true;
  });
  const unverifiedLocators = new Set(
    (citations ?? []).filter((c) => c.status === "unverified").map((c) => c.locator),
  );
  const unverifiedCount = (citations ?? []).filter(
    (c) => c.status === "unverified",
  ).length;
  const sourceKindLabels: Record<string, string> = {
    memory: t("pageInfo.sourceKind.memory"),
    chat: t("pageInfo.sourceKind.chat"),
    file: t("pageInfo.sourceKind.file"),
    obsidian: t("pageInfo.sourceKind.obsidian"),
    web: t("pageInfo.sourceKind.web"),
  };
  const sourceKindLabel = (mem: MemoryItem): string => {
    const key = mem.memory_type?.toLowerCase() ?? "";
    if (!key) return sourceKindLabels.memory;
    return KNOWN_SOURCE_KINDS.has(key) ? sourceKindLabels[key] : mem.memory_type!;
  };
  const agentLabel = (name: string | null | undefined): string =>
    name ? prettyAgent(name) : t("pageInfo.unknownAgent");
  const diagnosability =
    citationState === "cited"
      ? t("pageInfo.citationsLine", {
          total: (citations ?? []).length,
          unverified: unverifiedCount,
        })
      : citationState === "stripped-empty"
        ? t("pageInfo.citationsStrippedEmpty")
        : citationState === "stripped-mismatch"
          ? t("pageInfo.citationsStrippedMismatch")
          : null;
  const Wrapper = embedded ? "div" : "details";

  return (
    <Wrapper
      aria-label={embedded ? undefined : t("pageInfo.label")}
      className={embedded ? "note-info-page-groups" : "rounded-lg"}
      style={{ border: embedded ? "none" : "1px solid var(--mem-border)" }}
    >
      {!embedded && <summary
        className="flex items-center gap-2 px-4 py-3 cursor-pointer select-none list-none"
        style={{
          fontFamily: "var(--mem-font-mono)",
          fontSize: "11px",
          fontWeight: 600,
          letterSpacing: "0.05em",
          textTransform: "uppercase",
          color: "var(--mem-text-tertiary)",
        }}
      >
        <span>{t("pageInfo.label")}</span>
        <span style={{ fontWeight: 400, textTransform: "none", letterSpacing: 0 }}>
          {t("pageInfo.backlinks", { count: uniqueInbound.length })} ·{" "}
          {t("pageInfo.revisions", { count: revisions.length })} ·{" "}
          {basis && basis.length > 0
            ? basisCountLabel(t, sourceCount, basis.length)
            : t("pageInfo.sources", { count: sourceCount })}
        </span>
      </summary>}
      <div className={embedded ? "flex flex-col gap-4" : "flex flex-col gap-4 px-4 pb-4"}>
        {uniqueInbound.length > 0 && (
          <div>
            <h4 className={embedded ? "note-info-section-heading" : "mb-1"} style={embedded ? undefined : groupHeading}>
              {t("pageInfo.backlinks")}
            </h4>
            <div className={embedded ? "note-info-link-list" : "flex flex-wrap gap-1.5"}>
              {uniqueInbound.map((link) => (
                <button
                  key={link.source_page_id}
                  type="button"
                  onClick={() => onPageClick?.(link.source_page_id)}
                  className={embedded ? "note-info-link-row" : "rounded-md px-2.5 py-1.5 transition-colors duration-150 cursor-pointer hover:bg-[var(--mem-hover)]"}
                  style={embedded ? undefined : {
                    backgroundColor: "var(--mem-surface)",
                    border: "1px solid var(--mem-border)",
                    fontFamily: "var(--mem-font-body)",
                    fontSize: "12px",
                    fontWeight: 500,
                    color: "var(--mem-text)",
                  }}
                >
                  {embedded ? (
                    <span className="note-info-link-copy">
                      <PageIcon />
                      <span className="note-info-link-label">{link.label}</span>
                    </span>
                  ) : link.label}
                </button>
              ))}
            </div>
          </div>
        )}
        {(revisions.length > 0 || revisionsLoading || revisionsError) && (
          <div>
            <h4 className={embedded ? "note-info-section-heading" : "mb-1"} style={embedded ? undefined : groupHeading}>
              {t("pageInfo.revisions")}
            </h4>
            <div className="flex flex-col gap-1.5">
              {revisionsError && (
                <div
                  role="alert"
                  className="flex items-center gap-2 flex-wrap"
                  style={{
                    fontFamily: "var(--mem-font-body)",
                    fontSize: "13px",
                    color: "var(--mem-text-secondary)",
                  }}
                >
                  <span>{t("pageInfo.revisionsError")}</span>
                  <button type="button" disabled={revisionsLoading} style={{ ...showAllStyle, fontFamily: "var(--mem-font-body)", fontSize: "13px" }} onClick={onRetryRevisions}>
                    {t("pageInfo.retryRevisions")}
                  </button>
                </div>
              )}
              {!revisionsError && revisions.length === 0 && revisionsLoading && (
                <p
                  style={{
                    fontFamily: "var(--mem-font-body)",
                    fontSize: "13px",
                    color: "var(--mem-text-tertiary)",
                  }}
                >
                  {t("pageInfo.revisionsLoading")}
                </p>
              )}
              <ol className="note-revision-list" aria-label={t("pageInfo.revisions")}>
                {visibleRevisions.map((entry) => {
                  const incomingCount = entry.incoming_source_ids?.length ?? 0;
                  const summary = revisionSummary(t, entry, i18n.language);
                  return (
                    <li key={`${entry.version}-${entry.at}`} className="note-revision-entry">
                      <div className="note-revision-meta">
                        <span className="note-revision-version">v{entry.version}</span>
                        <span>{revisionWriterLabel(t, entry.edited_by, entry.agent)}</span>
                        <span className="note-revision-time">
                          {relativeMs(entry.at * 1000, relativeLocale)}
                        </span>
                      </div>
                      {summary && <p className="note-revision-summary">{summary}</p>}
                      {(incomingCount > 0 || entry.citations_summary) && (
                        <div className="note-revision-provenance">
                          {incomingCount > 0 && <span>{t("pageInfo.incoming", { count: incomingCount })}</span>}
                          {entry.citations_summary && <span>{entry.citations_summary}</span>}
                        </div>
                      )}
                    </li>
                  );
                })}
              </ol>
              {revisions.length > REVISIONS_SHOWN && (
                <button
                  type="button"
                  className="note-revision-toggle"
                  aria-expanded={showAllRevisions}
                  onClick={() => setShowAllRevisions((shown) => !shown)}
                >
                  {showAllRevisions
                    ? t("pageInfo.showLessRevisions")
                    : t("pageInfo.showAllRevisions", { count: revisions.length })}
                </button>
              )}
            </div>
          </div>
        )}
        {rows.length > 0 && (
          <div>
            <h4 className={embedded ? "note-info-section-heading" : "mb-1"} style={embedded ? undefined : groupHeading}>
              {t("pageInfo.sources")}
            </h4>
            <ul className={embedded ? "note-info-source-list" : undefined}>
              {visibleRows.map((row, idx) => {
                const mem = row.memory!;
                const locator = row.source.memory_source_id;
                return (
                  <li
                    key={locator}
                    data-testid="page-info-source-row"
                    className="py-2 px-2 transition-colors duration-150 hover:bg-[var(--mem-hover)]"
                    style={{
                      borderBottom:
                        embedded || idx === visibleRows.length - 1
                          ? "none"
                          : "1px solid color-mix(in srgb, var(--mem-border) 60%, transparent)",
                      cursor: "pointer",
                    }}
                  >
                    <button type="button" className="page-info-source-button" onClick={() => onMemoryClick(locator)}
                      aria-label={t("knowledgeContext.openMemory", { title: mem.title || t("knowledgeContext.untitled") })}>
                    <div className="flex items-center gap-2 flex-wrap">
                      <span
                        style={{
                          fontFamily: "var(--mem-font-mono)",
                          fontSize: "10px",
                          color: "var(--mem-text-tertiary)",
                          background: "var(--mem-hover)",
                          padding: "1px 5px",
                          borderRadius: "3px",
                          whiteSpace: "nowrap",
                        }}
                      >
                        {locator}
                      </span>
                      {mem.title && (
                        <span
                          className="truncate"
                          style={{
                            fontFamily: embedded ? "var(--mem-font-body)" : "var(--mem-font-heading)",
                            fontSize: "13px",
                            fontWeight: 500,
                            color: "var(--mem-text)",
                          }}
                        >
                          {mem.title}
                        </span>
                      )}
                    </div>
                    <div className="flex items-center gap-2 mt-0.5">
                      {mem.last_modified && (
                        <span
                          style={{
                            fontFamily: "var(--mem-font-body)",
                            fontSize: "11px",
                            color: "var(--mem-text-tertiary)",
                          }}
                        >
                          {relativeMs(mem.last_modified * 1000, relativeLocale)}
                        </span>
                      )}
                      <span
                        style={{
                          fontFamily: "var(--mem-font-body)",
                          fontSize: "11px",
                          color: "var(--mem-text-secondary)",
                        }}
                      >
                        {agentLabel(mem.source_agent)}
                      </span>
                      <span
                        style={{
                          fontFamily: "var(--mem-font-mono)",
                          fontSize: "10px",
                          color: "var(--mem-text-tertiary)",
                        }}
                      >
                        {sourceKindLabel(mem)}
                      </span>
                      {mem.version != null && mem.version > 1 && (
                        <span
                          style={{
                            fontFamily: "var(--mem-font-mono)",
                            fontSize: "10px",
                            color: "var(--mem-accent-blue, #60a5fa)",
                          }}
                        >
                          v{mem.version}
                        </span>
                      )}
                      {unverifiedLocators.has(locator) && (
                        <span
                          style={{
                            fontFamily: "var(--mem-font-mono)",
                            fontSize: "10px",
                            color: "var(--mem-accent-amber)",
                          }}
                        >
                          {t("citation.unverified")}
                        </span>
                      )}
                    </div>
                    </button>
                  </li>
                );
              })}
            </ul>
            {rows.length > SOURCES_SHOWN && !showAllSources && (
              <button
                type="button"
                style={showAllStyle}
                onClick={() => setShowAllSources(true)}
              >
                {t("pageInfo.showAllSources", { count: rows.length })}
              </button>
            )}
          </div>
        )}
        {onMarkBasis && onUnmarkBasis && currentPageId && (
          <PageInfoNotes
            pageId={currentPageId}
            space={currentSpace}
            basis={basis ?? []}
            onPageClick={onPageClick}
            onMarkBasis={onMarkBasis}
            onUnmarkBasis={onUnmarkBasis}
          />
        )}
        {diagnosability && (
          <p
            style={{
              fontFamily: "var(--mem-font-mono)",
              fontSize: "10px",
              color: "var(--mem-text-tertiary)",
            }}
          >
            {diagnosability}
          </p>
        )}
      </div>
    </Wrapper>
  );
}
