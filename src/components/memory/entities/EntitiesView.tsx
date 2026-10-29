// SPDX-License-Identifier: AGPL-3.0-only
import { useCallback, useEffect, useRef, useState } from "react";
import { TabEmptyMark } from "../../onboarding/TabIntro";
import { useTranslation } from "react-i18next";
import { Funnel } from "@phosphor-icons/react";
import { TopicIcon } from "./TopicIcon";
import { toast } from "sonner";
import { queryEntities, type Entity } from "../../../lib/tauri";
import { readAssetLens, writeAssetLens, type AssetLens } from "../../../lib/assetLens";
import { AssetLensToggle } from "../assets/AssetLensToggle";
import "../assets/assetCards.css";
import {
  DEFAULT_FILTERS,
  entityListRequest,
  filtersActive,
  type EntityFilters,
  type EntityTypeFilter,
} from "./entitiesViewModel";
import { loadActiveTopicsPage, mergeTopicRows, type TopicCursor } from "./topicBrowse";
import "./EntitiesView.css";

interface EntitiesViewProps {
  /** Opens the topic detail for every topic, including archived topics. */
  readonly onEntityClick: (entityId: string) => void;
}

const TYPE_CHIPS: EntityTypeFilter[] = ["all", "concept", "person", "organization", "place", "project", "technology"];

export function EntitiesView({ onEntityClick }: EntitiesViewProps) {
  const { t } = useTranslation();
  const [archived, setArchived] = useState(false);
  const [optionsOpen, setOptionsOpen] = useState(false);
  const [lens, setLens] = useState<AssetLens>(() => readAssetLens("entities"));
  const [filters, setFilters] = useState<EntityFilters>(DEFAULT_FILTERS);
  const [queryInput, setQueryInput] = useState("");
  const [entities, setEntities] = useState<Entity[]>([]);
  const [hasMore, setHasMore] = useState(false);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [loadError, setLoadError] = useState(false);
  const cursorRef = useRef<TopicCursor | undefined>(undefined);
  const offsetRef = useRef(0);
  const requestSeqRef = useRef(0);
  const pendingRef = useRef<number | null>(null);
  const loadedScopeRef = useRef<string | null>(null);
  const optionsRef = useRef<HTMLDivElement>(null);
  const optionsButtonRef = useRef<HTMLButtonElement>(null);
  const archivedOptionRef = useRef<HTMLButtonElement>(null);
  // Checking the current render's scope as well as the request id prevents a
  // response from landing between a scope change and its effect cleanup.
  const scope = JSON.stringify([archived, filters]);
  const scopeRef = useRef(scope);
  scopeRef.current = scope;

  useEffect(() => {
    const timeout = setTimeout(() => {
      setFilters((current) => current.query === queryInput ? current : { ...current, query: queryInput });
    }, 300);
    return () => clearTimeout(timeout);
  }, [queryInput]);

  const loadPage = useCallback(async (reset: boolean) => {
    if (!reset && (pendingRef.current !== null || loadedScopeRef.current !== scope)) return;
    const seq = ++requestSeqRef.current;
    pendingRef.current = seq;
    const nextOffset = reset ? 0 : offsetRef.current;
    const cursor = reset ? undefined : cursorRef.current;
    if (reset) {
      cursorRef.current = undefined;
      offsetRef.current = 0;
      loadedScopeRef.current = null;
      setEntities([]);
      setHasMore(false);
      setLoading(true);
      setLoadingMore(false);
      setLoadError(false);
    } else {
      setLoadingMore(true);
    }
    const isCurrent = () => seq === requestSeqRef.current && scopeRef.current === scope;
    try {
      const response = archived
        ? { ...await queryEntities(entityListRequest("archived", filters, nextOffset)), kind: "archived" as const }
        : { ...await loadActiveTopicsPage(filters, cursor), kind: "active" as const };
      if (!isCurrent()) return;
      setEntities((current) => mergeTopicRows(reset ? [] : current, response.entities));
      if (response.kind === "active") {
        cursorRef.current = response.cursor;
        setHasMore(response.hasMore);
      } else {
        offsetRef.current = nextOffset + response.entities.length;
        setHasMore(offsetRef.current < response.total && response.entities.length > 0);
      }
      loadedScopeRef.current = scope;
    } catch {
      if (!isCurrent()) return;
      if (reset) setLoadError(true);
      else toast.error(t("entities.error.load"));
    } finally {
      if (isCurrent()) {
        pendingRef.current = null;
        setLoading(false);
        setLoadingMore(false);
      }
    }
  }, [archived, filters, scope, t]);

  useEffect(() => {
    void loadPage(true);
    return () => {
      // Invalidates requests on filter/navigation changes and on unmount.
      ++requestSeqRef.current;
      pendingRef.current = null;
    };
  }, [loadPage]);

  useEffect(() => {
    if (!optionsOpen) return;
    archivedOptionRef.current?.focus();
    const handlePointerDown = (event: PointerEvent) => {
      if (!optionsRef.current?.contains(event.target as Node)) setOptionsOpen(false);
    };
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        setOptionsOpen(false);
        optionsButtonRef.current?.focus();
      }
    };
    document.addEventListener("pointerdown", handlePointerDown);
    document.addEventListener("keydown", handleKeyDown);
    return () => {
      document.removeEventListener("pointerdown", handlePointerDown);
      document.removeEventListener("keydown", handleKeyDown);
    };
  }, [optionsOpen]);

  const changeScope = (nextArchived: boolean) => {
    setOptionsOpen(false);
    setArchived(nextArchived);
  };

  const filtered = filtersActive(filters);
  const emptyTitle = filtered ? "entities.browse.noMatchesTitle"
    : archived ? "entities.empty.archivedTitle" : "entities.browse.emptyTitle";
  const emptyBody = filtered ? "entities.browse.noMatchesBody"
    : archived ? "entities.empty.archivedBody" : "entities.browse.emptyBody";

  return (
    <section aria-labelledby="entities-title" className={`entities-view${archived ? " entities-archived" : ""}`}>
      <header className={`entities-header${archived ? " entities-archive-context" : ""}`}>
        <div>
          <h1 className={archived ? "" : "sr-only"} id="entities-title">{t(archived ? "entities.browse.archivedTitle" : "entities.title")}</h1>
          {archived ? (
            <button className="entities-ghost-btn entities-back" onClick={() => changeScope(false)} type="button">
              <span aria-hidden="true">← </span>{t("entities.browse.backToTopics")}
            </button>
          ) : null}
        </div>
      </header>

      <div className="entities-toolbar collection-toolbar">
        <label className="collection-search">
          <Funnel size={18} aria-hidden="true" />
          <input
            aria-label={t("entities.search.label")}
            className="collection-search-input"
            onChange={(event) => setQueryInput(event.target.value)}
            placeholder={t("entities.search.placeholder")}
            type="search"
            value={queryInput}
          />
        </label>
        <div className="collection-toolbar-actions">
          <AssetLensToggle onChange={(next) => {
            setLens(next);
            writeAssetLens("entities", next);
          }} value={lens} />
          {!archived && <div className="entities-options" ref={optionsRef}>
            <button
              aria-controls="entities-options-menu"
              aria-expanded={optionsOpen}
              aria-haspopup="menu"
              aria-label={t("entities.browse.options")}
              className="entities-ghost-btn entities-options-button collection-control"
              onClick={() => setOptionsOpen((current) => !current)}
              ref={optionsButtonRef}
              type="button"
            >
              <span aria-hidden="true">…</span>
            </button>
            {optionsOpen && (
              <div className="entities-options-menu" id="entities-options-menu" role="menu">
                <button
                  className="entities-ghost-btn"
                  onClick={() => changeScope(true)}
                  ref={archivedOptionRef}
                  role="menuitem"
                  type="button"
                >
                  {t("entities.browse.showArchived")}
                </button>
              </div>
            )}
          </div>}
        </div>
      </div>

      <div className="entities-filters collection-filter-row">
        <div className="entities-chip-group" role="group" aria-label={t("entities.filters.typeLabel")}>
          <span className="entities-chip-label">{t("entities.filters.typeLabel")}</span>
          {TYPE_CHIPS.map((value) => (
            <button
              aria-pressed={filters.type === value}
              className="entities-chip collection-control"
              key={value}
              onClick={() => setFilters((current) => ({ ...current, type: value }))}
              type="button"
            >
              {value === "all" ? t("entities.filters.typeAny") : t(`entities.filters.type_${value}`)}
            </button>
          ))}
        </div>
      </div>

      {loading ? (
        <p className="entities-state" role="status">{t("entities.loading")}</p>
      ) : loadError ? (
        <div className="entities-state">
          <p role="alert">{t("entities.error.load")}</p>
          <button className="entities-ghost-btn" onClick={() => void loadPage(true)} type="button">
            {t("entityDetail.retry")}
          </button>
        </div>
      ) : entities.length === 0 ? (
        <div className="entities-empty">
          {!filtered && !archived ? <TabEmptyMark tab="topics" /> : null}
          <b>{t(emptyTitle)}</b>
          <p>{t(emptyBody)}</p>
        </div>
      ) : lens === "cards" ? (
        <div className="entities-list entities-list-cards" data-testid="entities-cards">
          {entities.map((entity) => (
            <button
              aria-label={entity.name}
              className="entity-row entity-row-card"
              key={entity.id}
              onClick={() => onEntityClick(entity.id)}
              data-testid={`entity-card-${entity.id}`}
              type="button"
            >
              <TopicIcon entityType={entity.entity_type} className="entity-row-icon" size={20} />
              <span className="entity-row-name">{entity.name}</span>
              <span className="entity-row-type">{t(`entities.filters.type_${entity.entity_type}`, { defaultValue: entity.entity_type })}</span>
              {entity.space && <span className="entity-row-space">{entity.space}</span>}
            </button>
          ))}
        </div>
      ) : (
        <div className="entities-list" data-testid="entities-list">
          {entities.map((entity) => (
            <button aria-label={entity.name} className="entity-row" key={entity.id} onClick={() => onEntityClick(entity.id)} type="button">
              <TopicIcon entityType={entity.entity_type} className="entity-row-icon" size={20} />
              <span className="entity-row-copy">
                <span className="entity-row-name">{entity.name}</span>
                <span className="entity-row-type">{t(`entities.filters.type_${entity.entity_type}`, { defaultValue: entity.entity_type })}</span>
              </span>
            </button>
          ))}
        </div>
      )}
      {!loading && !loadError && hasMore && (
        <button
          className="entities-ghost-btn entities-load-more"
          disabled={loadingMore}
          onClick={() => void loadPage(false)}
          type="button"
        >
          {t("entities.actions.loadMore")}
        </button>
      )}
    </section>
  );
}
