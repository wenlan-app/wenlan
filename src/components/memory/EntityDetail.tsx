// SPDX-License-Identifier: AGPL-3.0-only
import { InfoToggle } from "./page/InfoToggle";
import { WorkspaceBackButton } from "./navigation/WorkspaceNavigation";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useIsMutating, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import {
  archiveEntities,
  confirmEntity,
  changeEntityType,
  deleteEntity,
  getEntityDetail,
  restoreEntities,
  search,
  type EntityDetail as EntityDetailRecord,
} from "../../lib/tauri";
import { slotForEntityType } from "../../lib/graph/palette";
import { EntityConnections } from "./entity-detail/EntityConnections";
import { EntityContextRail } from "./entity-detail/EntityContextRail";
import "./entity-detail/EntityDetail.css";
import { formatRelativeEntityTime } from "./entity-detail/formatEntityMetadata";
import { EntityTopicMenu } from "./entity-detail/EntityTopicMenu";
import PageInfoDrawer from "./page/PageInfoDrawer";
import { DetailInfoContent, DetailReadingFrame } from "./DetailLayout";
import { useDetailInfoOpen } from "./useDetailInfoOpen";
import { InfoAction } from "./page/InfoAction";
import "./page/pageDocumentTools.css";
import "./context/knowledge-context.css";
import { EntityObservations } from "./entity-detail/EntityObservations";
import FocusGraph from "./FocusGraph";

const AtlasView = lazy(() => import("./AtlasView"));

interface EntityDetailProps {
  entityId: string;
  onBack: () => void;
  onEntityClick: (entityId: string) => void;
  onMemoryClick?: (sourceId: string) => void;
  onPageClick?: (pageId: string) => void;
  onRegisterFlush?: (flush: (() => Promise<boolean>) | null) => void;
}

export default function EntityDetail(props: EntityDetailProps) {
  // Navigation flushes the current topic before switching. Keep local editor,
  // dialog, and error state scoped to the newly mounted topic.
  return <EntityDetailContent key={props.entityId} {...props} />;
}

function EntityDetailContent({
  entityId,
  onBack,
  onEntityClick,
  onMemoryClick,
  onPageClick,
  onRegisterFlush,
}: EntityDetailProps) {
  const { t, i18n } = useTranslation();
  const queryClient = useQueryClient();
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [graphOpen, setGraphOpen] = useState(false);
  const { open: infoOpen, setOpen: setInfoOpen, toggle: toggleInfo } = useDetailInfoOpen(entityId);
  const infoToggleRef = useRef<HTMLButtonElement>(null);
  const observationsFlushRef = useRef<(() => Promise<boolean>) | null>(null);
  const actionGuardRef = useRef(false);
  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);
  const registerObservationsFlush = useCallback((flush: (() => Promise<boolean>) | null) => {
    observationsFlushRef.current = flush;
    onRegisterFlush?.(flush);
  }, [onRegisterFlush]);
  const afterObservationsSaved = async (action: () => void | Promise<unknown>): Promise<boolean> => {
    if (actionGuardRef.current) return false;
    actionGuardRef.current = true;
    try {
      const saved = await observationsFlushRef.current?.() ?? true;
      if (saved && mountedRef.current) {
        await action();
        return true;
      }
      return false;
    } finally { actionGuardRef.current = false; }
  };
  const notesPending = useIsMutating({ mutationKey: ["entity-observations", entityId] }) > 0;
  const locale = i18n.resolvedLanguage ?? i18n.language;
  const { data: detail, isError, refetch } = useQuery({
    queryKey: ["entityDetail", entityId],
    queryFn: () => getEntityDetail(entityId),
    refetchInterval: 5_000,
  });
  const { data: linkedMemories = [] } = useQuery({
    queryKey: ["entity-linked-memories", entityId, detail?.entity.name],
    queryFn: async () => {
      if (!detail?.entity.name) return [];
      const results = await search(detail.entity.name, 10, "memory");
      return results
        .filter((result) => result.entity_id === entityId || result.score > 0.7)
        .slice(0, 8);
    },
    enabled: Boolean(detail?.entity.name),
    staleTime: 30_000,
  });
  const invalidateEntityDetail = () => {
    queryClient.invalidateQueries({ queryKey: ["entityDetail", entityId] });
    queryClient.invalidateQueries({ queryKey: ["entities"] });
  };
  const invalidateEntityIndexes = () => Promise.all([
    "entities", "space-entities", "constellation-entities", "constellation-relations",
    "connections-entities", "searchEntities", "knowledge-graph", "constellation-cartography",
    "pages", "searchPages", "recent-concepts", "spaces-page-counts", "sidebar-space-page-counts",
  ].map((key) => queryClient.invalidateQueries({ queryKey: [key] })));
  const invalidateEntityLifecycle = () => Promise.all([
    // Other topic dossiers can still contain an edge to this topic.
    queryClient.invalidateQueries({ queryKey: ["entityDetail"] }),
    invalidateEntityIndexes(),
  ]);
  const confirmMutation = useMutation({
    mutationFn: (confirmed: boolean) => confirmEntity(entityId, confirmed),
    onSuccess: invalidateEntityLifecycle,
  });
  const typeMutation = useMutation({
    mutationFn: (entityType: string) => changeEntityType(entityId, entityType),
    onSuccess: invalidateEntityLifecycle,
  });
  const lifecycleMutation = useMutation({
    mutationFn: (archive: boolean) => (archive ? archiveEntities : restoreEntities)({
      ids: [entityId], dry_run: false,
    }),
    onSuccess: async () => {
      setConfirmDelete(false);
      await invalidateEntityLifecycle();
    },
  });
  const deleteMutation = useMutation({
    mutationFn: () => deleteEntity(entityId),
    onSuccess: () => {
      queryClient.removeQueries({ queryKey: ["entityDetail", entityId] });
      queryClient.removeQueries({ queryKey: ["entity-linked-memories", entityId] });
      queryClient.invalidateQueries({ queryKey: ["entities"] });
      queryClient.invalidateQueries({ queryKey: ["space-entities"] });
      queryClient.invalidateQueries({ queryKey: ["constellation-entities"] });
      queryClient.invalidateQueries({ queryKey: ["constellation-relations"] });
      queryClient.invalidateQueries({ queryKey: ["connections-entities"] });
      queryClient.invalidateQueries({ queryKey: ["searchEntities"] });
      onBack();
    },
  });
  const actionsPending = notesPending ||
    deleteMutation.isPending || lifecycleMutation.isPending || confirmMutation.isPending || typeMutation.isPending;

  if (!detail) {
    return (
      <div className="page-detail entity-detail-dossier" aria-label={t("entityDetail.dossierLabel")}>
        <header className="entity-dossier-header">
          <BackButton onBack={onBack} label={t("entityDetail.back")} />
        </header>
        <div className="entity-detail-status" role="status">
          {isError ? (
            <>
              <p className="entity-empty">{t("entityDetail.loadError")}</p>
              <button
                type="button"
                className="memory-detail-text-button"
                onClick={() => refetch()}
              >
                {t("entityDetail.retry")}
              </button>
            </>
          ) : (
            <p className="entity-empty">{t("entityDetail.loading")}</p>
          )}
        </div>
      </div>
    );
  }

  const { entity, observations, relations } = detail;
  const archived = entity.status === "archived";
  return (
    <div
      className={`page-detail entity-detail-dossier document-context-host${infoOpen ? " document-context-open" : ""}`}
      aria-label={t("entityDetail.dossierLabel")}
    >
      <DetailReadingFrame variant="compact" className="page-detail-document">
        <header className="page-document-title-row">
          <h1 className="page-detail-title">{entity.name}</h1>
          <div className="page-document-tools">
            <BackButton onBack={onBack} label={t("entityDetail.back")} />
            <InfoToggle ref={infoToggleRef} open={infoOpen} onClick={toggleInfo} />
            <EntityTopicMenu
              archived={archived}
              actionsPending={actionsPending}
              onArchive={() => { void afterObservationsSaved(() => lifecycleMutation.mutate(!archived)); }}
              onDelete={() => { void afterObservationsSaved(() => setConfirmDelete(true)); }}
            />
          </div>
        </header>
        {archived ? <p className="entity-archived-notice" role="status">{t("entityDetail.archived")}</p> : null}
        {deleteMutation.isError || lifecycleMutation.isError ? (
          <p className="entity-error" role="alert">{t("entityDetail.saveError")}</p>
        ) : null}
        {confirmDelete ? (
          <div className="entity-delete-confirmation">
            <span className="entity-delete-question">{t("entityDetail.deleteQuestion")}</span>
            <button type="button" disabled={actionsPending} onClick={() => { void afterObservationsSaved(() => deleteMutation.mutate()); }} className="entity-delete-confirm">
              {t("entityDetail.delete")}
            </button>
            <button type="button" disabled={actionsPending} onClick={() => setConfirmDelete(false)} className="memory-detail-text-button">
              {t("entityDetail.cancel")}
            </button>
          </div>
        ) : null}
        <section className="page-detail-prose entity-detail-reading" aria-label={t("entityDetail.readingLabel")}>
          <EntityObservations
            key={`${entityId}:${archived}`}
            entityId={entityId}
            entityName={entity.name}
            observations={observations}
            onInvalidate={invalidateEntityDetail}
            readOnly={archived}
            blocked={deleteMutation.isPending || lifecycleMutation.isPending || confirmMutation.isPending || typeMutation.isPending}
            onRegisterFlush={registerObservationsFlush}
          />
        </section>
      </DetailReadingFrame>
      <PageInfoDrawer docked inspector focusOnOpen={false} returnFocusRef={infoToggleRef} open={infoOpen} onClose={() => setInfoOpen(false)} title={t("entityDetail.contextTitle")} closeLabel={t("common.close")}>
        <DetailInfoContent className="entity-topic-context">
          <InfoAction
            label={t(entity.confirmed ? "infoActions.releaseTopic" : "infoActions.keepTopic")}
            description={t(archived ? "infoActions.restoreTopicFirst" : entity.confirmed ? "infoActions.topicKeptHint" : "infoActions.keepTopicHint")}
            active={entity.confirmed} pending={confirmMutation.isPending} disabled={archived || actionsPending}
            error={confirmMutation.isError ? t("infoActions.saveError") : undefined}
            onClick={() => { void afterObservationsSaved(() => confirmMutation.mutate(!entity.confirmed)); }}
          />
          <EntityContextRail
            entity={entity}
            locale={locale}
            onMemoryClick={onMemoryClick}
            archived={archived}
            typePending={typeMutation.isPending || actionsPending}
            onTypeChange={(entityType) => afterObservationsSaved(() => typeMutation.mutateAsync(entityType))}
          >
            <EntityConnections
              name={entity.name}
              relations={relations}
              onEntityClick={onEntityClick}
              onExpand={() => setGraphOpen(true)}
            />
          </EntityContextRail>
          {graphOpen ? (
            <EntityGraphOverlay
              detail={detail}
              linkedMemoriesCount={linkedMemories.length}
              locale={locale}
              onClose={() => setGraphOpen(false)}
              onEntityClick={onEntityClick}
              onMemoryClick={onMemoryClick}
              onPageClick={onPageClick}
            />
          ) : null}
        </DetailInfoContent>
      </PageInfoDrawer>
    </div>
  );
}

function BackButton({ onBack, label }: { readonly onBack: () => void; readonly label: string }) {
  return (
    <WorkspaceBackButton
      type="button"
      onClick={onBack}
      className="memory-detail-back"
      aria-label={label}
      title={label}
    >
      <svg
        aria-hidden="true"
        width="16"
        height="16"
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.5"
      >
        <path d="M19 12H5M12 19l-7-7 7-7" />
      </svg>
    </WorkspaceBackButton>
  );
}

type EntityGraphOverlayProps = {
  readonly detail: EntityDetailRecord;
  readonly linkedMemoriesCount: number;
  readonly locale: string;
  readonly onClose: () => void;
  readonly onEntityClick: (entityId: string) => void;
  readonly onMemoryClick?: (sourceId: string) => void;
  readonly onPageClick?: (pageId: string) => void;
};

function EntityGraphOverlay({
  detail,
  linkedMemoriesCount,
  locale,
  onClose,
  onEntityClick,
  onMemoryClick,
  onPageClick,
}: EntityGraphOverlayProps) {
  const { t } = useTranslation();
  const overlayRef = useRef<HTMLDivElement>(null);
  const closeRef = useRef<HTMLButtonElement>(null);
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;
  useEffect(() => {
    const trigger = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    closeRef.current?.focus();
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.isComposing || event.defaultPrevented) return;
      if (event.key === "Escape") {
        event.preventDefault();
        event.stopPropagation();
        onCloseRef.current();
      } else if (event.key === "Tab") {
        // Own every Tab step: Safari may otherwise skip buttons according to
        // system keyboard settings and move focus out of this modal.
        event.preventDefault();
        event.stopPropagation();
        const controls = Array.from(overlayRef.current?.querySelectorAll<HTMLElement>(
          'button:not(:disabled), a[href], input:not(:disabled), select:not(:disabled), textarea:not(:disabled), [tabindex]:not([tabindex="-1"])',
        ) ?? []).filter((control) => control.tabIndex >= 0 && control.getClientRects().length > 0 && getComputedStyle(control).visibility !== "hidden");
        const active = document.activeElement;
        const index = controls.findIndex((control) => control === active);
        const ordered = event.shiftKey ? [...controls].reverse() : controls;
        const next = index >= 0
          ? controls[(index + (event.shiftKey ? -1 : 1) + controls.length) % controls.length]
          : ordered.find((control) => active && (
            active.compareDocumentPosition(control) & (
              event.shiftKey ? Node.DOCUMENT_POSITION_PRECEDING : Node.DOCUMENT_POSITION_FOLLOWING
            )
          )) ?? ordered[0];
        next?.focus();
      }
    };
    // Inner Atlas popovers handle Escape at document capture first. The graph
    // then owns bubbling keys before the parent context drawer sees them.
    const overlay = overlayRef.current;
    overlay?.addEventListener("keydown", onKeyDown);
    return () => {
      overlay?.removeEventListener("keydown", onKeyDown);
      if (trigger?.isConnected) trigger.focus();
    };
  }, []);
  const [mode, setMode] = useState<"focus" | "map">("focus");
  const [showVerbs, setShowVerbs] = useState(true);
  const { entity, relations } = detail;
  const neighborCount = useMemo(
    () =>
      new Set(
        relations
          .filter((relation) => relation.entity_id !== entity.id)
          .map((relation) => relation.entity_id),
      ).size,
    [entity.id, relations],
  );
  const openEntity = (entityId: string) => {
    onClose();
    onEntityClick(entityId);
  };

  return (
    <div
      ref={overlayRef}
      role="dialog"
      aria-modal="true"
      aria-label={t("entityDetail.expandGraph")}
      className="fixed inset-0 z-50"
      style={{
        background: "var(--mem-bg)",
        display: "flex",
        flexDirection: "column",
      }}
    >
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "10px 16px",
          borderBottom: "1px solid var(--mem-border)",
          background: "var(--mem-surface)",
          fontFamily: "var(--mem-font-body)",
          flexWrap: "wrap",
        }}
      >
        <button
          type="button"
          ref={closeRef}
          onClick={onClose}
          className="memory-detail-icon-button"
          aria-label={t("common.close")}
          title={t("common.close")}
        >
          <svg
            aria-hidden="true"
            width="16"
            height="16"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
          >
            <path d="M18 6 6 18M6 6l12 12" />
          </svg>
        </button>
        <span style={{ fontSize: "var(--mem-text-meta)", color: "var(--mem-text-tertiary)" }}>
          {t("focus.crumbAtlas")}
          {mode === "focus" ? (
            <>
              {" ▸ "}
              <b style={{ color: "var(--mem-text)", fontWeight: 500 }}>
                {t("focus.crumbFocus", { name: entity.name })}
              </b>
            </>
          ) : null}
        </span>
        {mode === "focus" ? (
          <button
            type="button"
            aria-pressed={showVerbs}
            onClick={() => setShowVerbs((value) => !value)}
            style={{
              fontSize: "var(--mem-text-control)",
              color: showVerbs ? "var(--mem-text)" : "var(--mem-text-secondary)",
              border: `1px solid ${
                showVerbs ? "var(--mem-distilled-border)" : "var(--mem-border)"
              }`,
              borderRadius: "var(--mem-radius-full)",
              padding: "4px 12px",
              background: showVerbs ? "var(--mem-indigo-bg)" : "transparent",
              cursor: "pointer",
              fontFamily: "inherit",
            }}
          >
            {t("focus.showVerbs")}
          </button>
        ) : null}
        {mode === "focus" ? (
          <span
            style={{
              marginLeft: "auto",
              font: "400 11px var(--mem-font-mono)",
              color: "var(--mem-text-tertiary)",
            }}
          >
            {t("focus.neighbors", { count: neighborCount })}
            {linkedMemoriesCount > 0
              ? ` · ${t("focus.memoriesCount", { count: linkedMemoriesCount })}`
              : ""}
          </span>
        ) : null}
        <div
          role="group"
          aria-label={t("focus.viewSegmentLabel")}
          style={{
            display: "flex",
            border: "1px solid var(--mem-border)",
            borderRadius: "var(--mem-radius-md)",
            overflow: "hidden",
            marginLeft: mode === "focus" ? 0 : "auto",
          }}
        >
          {(["map", "focus"] as const).map((nextMode) => {
            const selected = mode === nextMode;
            return (
              <button
                key={nextMode}
                type="button"
                aria-pressed={selected}
                onClick={() => setMode(nextMode)}
                style={{
                  fontSize: "var(--mem-text-control)",
                  padding: "4px 14px",
                  border: "none",
                  cursor: "pointer",
                  fontFamily: "inherit",
                  color: selected ? "var(--mem-text)" : "var(--mem-text-tertiary)",
                  fontWeight: selected ? 500 : 400,
                  background: selected ? "var(--mem-hover-strong)" : "transparent",
                }}
              >
                {nextMode === "map" ? t("focus.segAtlas") : t("focus.segFocus")}
              </button>
            );
          })}
        </div>
      </div>
      <div
        style={{
          flex: 1,
          minHeight: 0,
          position: "relative",
          ...(mode === "focus"
            ? { display: "grid", gridTemplateColumns: "minmax(0, 1fr) 300px" }
            : {}),
        }}
      >
        {mode === "focus" ? (
          <>
            <div style={{ position: "relative", minWidth: 0 }}>
              <FocusGraph
                detail={detail}
                onEntityClick={openEntity}
                fill
                showVerbs={showVerbs}
                memoriesCount={linkedMemoriesCount}
              />
            </div>
            <aside
              aria-label={t("focus.panelLabel")}
              style={{
                borderLeft: "1px solid var(--mem-border)",
                background: "var(--mem-surface)",
                padding: 18,
                overflowY: "auto",
                fontFamily: "var(--mem-font-body)",
              }}
            >
              <div
                style={{
                  font: "500 10px var(--mem-font-mono)",
                  letterSpacing: "0.14em",
                  textTransform: "uppercase",
                  color: "var(--mem-text-tertiary)",
                }}
              >
                <i
                  aria-hidden="true"
                  style={{
                    color: `var(--kg-${slotForEntityType(entity.entity_type)})`,
                    fontStyle: "normal",
                  }}
                >
                  ●
                </i>
                {` ${entity.entity_type} · ${
                  entity.status === "archived"
                    ? t("entityDetail.archived")
                    : entity.confirmed
                    ? t("focus.confirmedState")
                    : t("focus.unconfirmedState")
                }`}
              </div>
              <h4
                style={{
                  fontSize: 22,
                  fontWeight: 600,
                  margin: "6px 0 2px",
                  color: "var(--mem-text)",
                }}
              >
                {entity.name}
              </h4>
              <div style={{ fontSize: "var(--mem-text-meta)", color: "var(--mem-text-tertiary)" }}>
                {t("focus.observations", { count: detail.observations.length })}
                {` · ${t("focus.relations", { count: relations.length })}`}
                {` · ${t("focus.updatedAgo", {
                  ago: formatRelativeEntityTime(entity.updated_at, locale) ?? "",
                })}`}
              </div>
              <hr
                style={{
                  border: "none",
                  borderTop: "1px solid var(--mem-detail-divider)",
                  margin: "14px 0",
                }}
              />
              <div>
                {relations.map((relation) => (
                  <button
                    key={relation.id}
                    type="button"
                    onClick={() => openEntity(relation.entity_id)}
                    style={{
                      display: "flex",
                      alignItems: "baseline",
                      gap: 8,
                      padding: "5px 0",
                      fontSize: 12.5,
                      width: "100%",
                      background: "none",
                      border: "none",
                      cursor: "pointer",
                      textAlign: "left",
                      fontFamily: "inherit",
                    }}
                  >
                    <span
                      aria-hidden="true"
                      style={{
                        width: 7,
                        height: 7,
                        borderRadius: "var(--mem-radius-full)",
                        flex: "none",
                        alignSelf: "center",
                        background: `var(--kg-${slotForEntityType(relation.entity_type)})`,
                      }}
                    />
                    <code
                      style={{
                        font: "500 10px var(--mem-font-mono)",
                        color: "var(--mem-text-tertiary)",
                        letterSpacing: "0.04em",
                        minWidth: 96,
                      }}
                    >
                      {relation.direction === "incoming"
                        ? `${relation.relation_type} ←`
                        : `${relation.relation_type} →`}
                    </code>
                    <span style={{ color: "var(--mem-text)" }}>
                      {relation.entity_name}
                    </span>
                  </button>
                ))}
              </div>
              <hr
                style={{
                  border: "none",
                  borderTop: "1px solid var(--mem-detail-divider)",
                  margin: "14px 0",
                }}
              />
              <button
                type="button"
                onClick={onClose}
                style={{
                  display: "inline-block",
                  font: "500 12.5px var(--mem-font-body)",
                  color: "var(--mem-text)",
                  background: "var(--mem-indigo-bg)",
                  border: "1px solid var(--mem-distilled-border)",
                  borderRadius: "var(--mem-radius-md)",
                  padding: "7px 14px",
                  cursor: "pointer",
                }}
              >
                {t("focus.openEntity")}
              </button>
            </aside>
          </>
        ) : (
          <Suspense fallback={null}>
            <AtlasView
              focusEntityId={entity.id}
              onNodeClick={(target) => {
                if (target.kind === "entity") {
                  openEntity(target.id);
                } else if (target.kind === "page") {
                  // The overlay has no page router of its own; close it and
                  // let the host navigate, the same shape a memory click uses.
                  if (onPageClick) {
                    onClose();
                    onPageClick(target.id);
                  }
                } else if (onMemoryClick) {
                  onClose();
                  onMemoryClick(target.id);
                }
              }}
            />
          </Suspense>
        )}
      </div>
    </div>
  );
}
