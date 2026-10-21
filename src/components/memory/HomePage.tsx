// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useMemo, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";
import {
  getActiveImportBatches,
  type Page,
} from "../../lib/tauri";
import { ImportDetailPanel, ImportStatusPill } from "./ImportPhases";
import { isKnowledgePage, listAllActivePages } from "./pages/listAllPages";
import { FirstPageMilestone } from "../onboarding/FirstPageMilestone";
import "./homeEmptyState.css";
import { useSteepOnDemand } from "../../lib/steepMode";

interface HomePageProps {
  onNavigateGraph: () => void;
  onSelectPage?: (pageId: string) => void;
  /** Kept for existing first-use routing; Home no longer displays its entry. */
  onStartFirstUse?: () => void;
  /**
   * Starts the same new-page draft flow the Wiki overview's New page action
   * uses. Required: the empty state offers "Write a page" in every variant, so
   * the copy must never name an action with no handler behind it.
   */
  onCreatePage: (space: string | null) => void;
  /**
   * Opens Settings → Intelligence, where a local model is installed or an API
   * key is set. Required for the same reason as {@link onCreatePage}.
   */
  onOpenIntelligenceSettings: () => void;
}

export default function HomePage({
  onNavigateGraph: _onNavigateGraph,
  onSelectPage,
  onCreatePage,
  onOpenIntelligenceSettings,
}: HomePageProps) {
  const {
    data: recentConcepts = [],
    isLoading: recentConceptsLoading,
    isFetched: recentConceptsFetched,
  } = useQuery({
    queryKey: ["recent-concepts"],
    queryFn: listAllActivePages,
    refetchInterval: 10_000,
  });

  // The daemon's browse list carries an entity "shadow" page for every entity
  // by contract. Those belong in Wiki browse but are not pages anyone wrote or
  // Wenlan distilled, so filtering once here keeps the empty-state switch and
  // the home page list counting the same thing.
  const knowledgePages = useMemo(
    () => recentConcepts.filter(isKnowledgePage),
    [recentConcepts],
  );

  const recentlyRefinedPages = useMemo(
    () =>
      [...knowledgePages]
        .sort((a, b) => Date.parse(b.last_modified) - Date.parse(a.last_modified))
        .slice(0, 6),
    [knowledgePages],
  );

  // The "recent-concepts" query has never resolved right after onboarding
  // (HomePage isn't mounted during the wizard), so its default `[]` would
  // otherwise be read as "no pages" and flash the empty state before the real
  // page list arrives. Once the first attempt has answered, keep the rest of
  // Home mounted during background refetches to avoid replacing the page list
  // with the empty state during an ambient page-list failure.
  if (recentConceptsLoading && !recentConceptsFetched) {
    return null;
  }

  return (
    <>
      <WikiHome
        knowledgePages={knowledgePages}
        pages={recentlyRefinedPages}
        onSelectPage={onSelectPage}
        onCreatePage={onCreatePage}
        onOpenIntelligenceSettings={onOpenIntelligenceSettings}
      />

      <FirstPageMilestone pages={recentConcepts} onSelectPage={onSelectPage} />
    </>
  );
}


function formatSourceCount(t: TFunction, count: number): string {
  return t("home.counts.source", { count });
}

function relativePageDate(t: TFunction, value: string): string {
  const ms = Date.parse(value);
  if (Number.isNaN(ms)) return t("home.relative.updatedRecently");
  const delta = Date.now() - ms;
  const days = Math.floor(delta / 86_400_000);
  if (days <= 0) return t("home.relative.today");
  if (days === 1) return t("home.relative.yesterday");
  if (days < 7) return t("home.relative.daysAgo", { count: days });
  const weeks = Math.floor(days / 7);
  return weeks === 1 ? t("home.relative.weekAgo") : t("home.relative.weeksAgo", { count: weeks });
}

function useElementMinWidth<T extends HTMLElement>(minWidth: number) {
  const ref = useRef<T | null>(null);
  const [matches, setMatches] = useState(false);

  function getMatches() {
    if (typeof window === "undefined") return false;
    const width = ref.current?.getBoundingClientRect().width ?? window.innerWidth;
    return width >= minWidth;
  }

  useEffect(() => {
    if (typeof window === "undefined") return;
    const element = ref.current;
    const update = () => setMatches(getMatches());
    update();

    if (element && "ResizeObserver" in window) {
      const observer = new ResizeObserver(update);
      observer.observe(element);
      return () => observer.disconnect();
    }

    window.addEventListener("resize", update);
    return () => window.removeEventListener("resize", update);
  }, [minWidth]);

  return [ref, matches] as const;
}

function WikiHome({
  knowledgePages,
  pages,
  onSelectPage,
  onCreatePage,
  onOpenIntelligenceSettings,
}: {
  /** Active pages minus entity shadow pages — what a person calls "my pages". */
  knowledgePages: Page[];
  pages: Page[];
  onSelectPage?: (pageId: string) => void;
  onCreatePage: (space: string | null) => void;
  onOpenIntelligenceSettings: () => void;
}) {
  const [containerRef, isWideLayout] = useElementMinWidth<HTMLDivElement>(820);
  const [importDetailOpen, setImportDetailOpen] = useState(false);
  const steepOnDemand = useSteepOnDemand() === true;
  // Background import phases, if any. Asked here — not higher — because this
  // is the only surface whose copy branches on the answer, and the command
  // rejects on flavors that do not serve it: a rejection just means no pill.
  const { data: activeImports } = useQuery({
    queryKey: ["active-import-batches"],
    queryFn: getActiveImportBatches,
    refetchInterval: 10_000,
  });
  const importBatches = activeImports?.batches ?? [];
  // Close the panel when the last batch settles. Left open, it stays armed:
  // the next import would replace the whole home grid with a detail panel the
  // user never asked for, minutes or days after they last opened one.
  const hasActiveImports = importBatches.length > 0;
  useEffect(() => {
    if (!hasActiveImports) setImportDetailOpen(false);
  }, [hasActiveImports]);
  return (
    <div
      data-testid="wiki-home"
      ref={containerRef}
      className="wiki-home"
      style={{
        display: "grid",
        gap: isWideLayout ? 24 : 22,
        gridTemplateColumns: "minmax(0, 1fr)",
        maxWidth: 1280,
        margin: "0 auto",
        width: "100%",
        paddingBottom: 64,
        alignItems: "start",
      }}
    >
      <section
        data-testid="wiki-daily-desk"
        className="wiki-daily-desk"
      >
        <TodayHeader
          statusPill={<ImportStatusPill batches={importBatches} onOpen={() => setImportDetailOpen(true)} />}
        />
      </section>

      {importDetailOpen && importBatches.length > 0 ? (
        <div style={{ gridColumn: "1 / -1", minWidth: 0 }}>
          <ImportDetailPanel batches={importBatches} onBack={() => setImportDetailOpen(false)} onDemand={steepOnDemand} />
        </div>
      ) : (
      <div
        data-testid="wiki-content-grid"
        className="wiki-content-grid"
        style={{
          display: "grid",
          gap: isWideLayout ? 28 : 24,
          gridTemplateColumns: "minmax(0, 1fr)",
          gridColumn: "1 / -1",
          alignItems: "start",
          minWidth: 0,
        }}
      >
        {knowledgePages.length === 0 ? (
          <HomeEmptyState
            onCreatePage={onCreatePage}
            onOpenIntelligenceSettings={onOpenIntelligenceSettings}
          />
        ) : (
          <PageList
            pages={pages}
            onSelectPage={onSelectPage}
            isWideLayout={isWideLayout}
          />
        )}
      </div>
      )}

    </div>
  );
}

function SectionHeading({
  title,
  action,
  size = "default",
  level = 2,
}: {
  title: string;
  action?: React.ReactNode;
  size?: "default" | "compact" | "page";
  level?: 1 | 2;
}) {
  const Heading = level === 1 ? "h1" : "h2";
  const pageHeading = size === "page";
  return (
    <div
      style={{
        display: "flex",
        alignItems: "baseline",
        justifyContent: "space-between",
        gap: 12,
        marginBottom: pageHeading ? 16 : 12,
      }}
    >
      <Heading
        style={{
          fontFamily: "var(--mem-font-heading)",
          fontSize: size === "compact" ? 14 : pageHeading ? "var(--mem-destination-title-size)" : 18,
          fontWeight: 500,
          color: "var(--mem-text)",
          letterSpacing: pageHeading ? "-0.03em" : 0,
          lineHeight: pageHeading ? 1.12 : 1.2,
          margin: 0,
        }}
      >
        {title}
      </Heading>
      {action}
    </div>
  );
}

function TodayHeader({ statusPill }: { statusPill?: React.ReactNode }) {
  const { t } = useTranslation();
  return (
    <section data-testid="wiki-today-heading" className="wiki-today-heading">
      <SectionHeading
        title={t("home.todayInWenlan")}
        level={1}
        size="page"
        action={statusPill}
      />
    </section>
  );
}

const EMPTY_ACTION_STYLE: React.CSSProperties = {
  fontFamily: "var(--mem-font-body)",
  fontSize: "var(--mem-text-control)",
  borderRadius: 8,
  padding: "7px 12px",
  cursor: "pointer",
};

/** Notes are usable immediately; AI setup is an optional next step. */
function HomeEmptyState({
  onCreatePage,
  onOpenIntelligenceSettings,
}: {
  onCreatePage: (space: string | null) => void;
  onOpenIntelligenceSettings: () => void;
}) {
  const { t } = useTranslation();
  return (
    <section data-testid="wiki-page-empty" aria-labelledby="wiki-page-empty-title">
      <h2 id="wiki-page-empty-title" style={{
        fontFamily: "var(--mem-font-heading)", fontSize: 18, fontWeight: 500,
        color: "var(--mem-text)", lineHeight: 1.4, margin: "0 0 10px",
      }}>
        {t("home.empty.noPages")}
      </h2>
      <p style={{ fontFamily: "var(--mem-font-body)", fontSize: 14,
        color: "var(--mem-text-secondary)", lineHeight: 1.6, margin: "0 0 20px" }}>
        {t("home.empty.notesFirst")}
      </p>
      <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 16 }}>
        <button type="button" className="home-empty-action" onClick={() => onCreatePage(null)}
          style={{ ...EMPTY_ACTION_STYLE, border: "1px solid var(--mem-border)",
            background: "var(--mem-surface)", color: "var(--mem-text)" }}>
          {t("home.empty.writePage")}
        </button>
        <button type="button" className="home-empty-action" onClick={onOpenIntelligenceSettings}
          style={{ ...EMPTY_ACTION_STYLE, padding: "7px 0", border: "none",
            background: "none", color: "var(--mem-text-secondary)" }}>
          {t("home.empty.aiOptional")}
        </button>
      </div>
    </section>
  );
}

function PageList({
  pages,
  onSelectPage,
  isWideLayout,
}: {
  pages: Page[];
  onSelectPage?: (pageId: string) => void;
  isWideLayout: boolean;
}) {
  const { t } = useTranslation();
  if (!pages.length) return null;
  return (
    <div>
      <div
        data-testid="wiki-page-list"
        style={{
          display: "grid",
          gap: 0,
          borderTopStyle: "none",
          borderTopWidth: 0,
          borderTopColor: "transparent",
        }}
      >
        {pages.map((page) => (
          <button
            key={page.id}
            type="button"
            aria-label={t("home.openPage", { title: page.title })}
            className="transition-colors duration-150 hover:bg-[var(--mem-hover)]"
            style={{
              display: "grid",
              width: "100%",
              gap: isWideLayout ? 20 : 12,
              gridTemplateColumns: isWideLayout
                ? "minmax(240px, 1fr) minmax(128px, auto)"
                : "minmax(0, 1fr)",
              padding: isWideLayout ? "14px 4px" : "15px 4px",
              textAlign: "left",
              border: "none",
              borderBottom: "1px solid color-mix(in srgb, var(--mem-border) 70%, transparent)",
              background: "transparent",
              color: "inherit",
              cursor: onSelectPage ? "pointer" : "default",
            }}
            onClick={() => onSelectPage?.(page.id)}
          >
            <div style={{ display: "flex", minWidth: 0, gap: 12 }}>
              <PageIcon />
              <div className="min-w-0">
                <p
                  style={{
                    display: "-webkit-box",
                    WebkitLineClamp: 2,
                    WebkitBoxOrient: "vertical",
                    overflow: "hidden",
                    fontFamily: "var(--mem-font-heading)",
                    fontSize: 16,
                    fontWeight: 500,
                    color: "var(--mem-text)",
                    lineHeight: 1.18,
                    margin: 0,
                  }}
                >
                  {page.title}
                </p>
                <p
                  className="truncate"
                  style={{
                    fontFamily: "var(--mem-font-mono)",
                    fontSize: "var(--mem-text-meta)",
                    color: "var(--mem-text-tertiary)",
                    margin: "6px 0 0",
                  }}
                >
                  {page.space?.trim() || page.domain?.trim() || t("pages.overview.title")}
                </p>
                {page.summary && (
                  <p
                    style={{
                      display: isWideLayout ? "none" : "-webkit-box",
                      WebkitLineClamp: 2,
                      WebkitBoxOrient: "vertical",
                      overflow: "hidden",
                      fontFamily: "var(--mem-font-body)",
                      fontSize: "var(--mem-text-meta)",
                      color: "var(--mem-text-secondary)",
                      lineHeight: 1.45,
                      margin: "8px 0 0",
                    }}
                  >
                    {page.summary}
                  </p>
                )}
              </div>
            </div>

            <div
              style={{
                display: "flex",
                flexDirection: isWideLayout ? "column" : "row",
                flexWrap: "wrap",
                alignItems: isWideLayout ? "flex-end" : "center",
                justifyContent: isWideLayout ? "center" : "flex-start",
                gap: isWideLayout ? 4 : 12,
                fontFamily: "var(--mem-font-body)",
                fontSize: "var(--mem-text-meta)",
                color: "var(--mem-text-tertiary)",
                textAlign: isWideLayout ? "right" : "left",
              }}
            >
              <span>{formatSourceCount(t, page.source_memory_ids.length)}</span>
              <span>{relativePageDate(t, page.last_modified)}</span>
            </div>
          </button>
        ))}
      </div>
    </div>
  );
}

function PageIcon() {
  return (
    <svg width="15" height="15" viewBox="0 0 24 24" fill="none" aria-hidden="true" className="mt-1.5 shrink-0" style={{ color: "var(--mem-page-icon)" }}>
      <path d="M12 2L2 7l10 5 10-5-10-5zM2 17l10 5 10-5M2 12l10 5 10-5" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" />
    </svg>
  );
}
