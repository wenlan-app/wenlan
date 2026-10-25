// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getMemoryDetail, getPage, type MemoryItem, type Page, type PageCitation } from "../../../lib/tauri";
import { citationDisplayLabel, citationExternalDetail } from "../../../lib/pageCitations";
import { ReferenceTypeIcon, citationReferenceKind, citationSourceApp, type ReferenceKind } from "./ReferenceTypeIcon";
import { sourceAppName } from "../sources/sourcePresentation";
import type { NavigableReferenceTarget, ReferencePreviewRequest } from "./referenceTypes";
import { openCitationTarget } from "./openCitationTarget";
import { useReferencePopoverPosition } from "./useReferencePopoverPosition";
import { useReferencePreviewKeyboard } from "./useReferencePreviewKeyboard";
import { relativeMs } from "../page/format";
import { citationFilePath } from "./openCitationTarget";

export interface ReferencePreviewProps {
  request: ReferencePreviewRequest | null;
  onDismiss(): void;
  onOpenPage?(pageId: string): void;
  onOpenMemory?(sourceId: string): void;
  onOpen?(target: NavigableReferenceTarget): void;
  onOpenCitationTarget?(citation: PageCitation): void;
  onPointerEnter(): void;
  onPointerLeave(): void;
  onEscape?(): void;
  role?: "dialog" | "tooltip";
  dataPageLinkPreview?: boolean;
  dataCitationPopover?: boolean;
  id?: string;
  copyPrefix?: "reference" | "pageLinkPreview";
  externalOpenFailure?: string | null;
}

const WIDTH = 300;

export function referenceExcerpt(raw: string): string {
  return raw
    .replace(/^---\r?\n[\s\S]*?\r?\n(?:---|\.\.\.)\s*\r?\n/, "")
    .replace(/^\s*#\s+[^\n]+\n+/, "")
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/<[^>]*>/g, " ")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\[\[([^\]|#]+)(?:#[^\]|]+)?(?:\|([^\]]+))?\]\]/g, (_all, target: string, alias?: string) => alias?.trim() || target.trim())
    // A heading becomes its own sentence, so it does not run into the paragraph under it.
    .replace(/^\s{0,3}#{1,6}\s+(.+?)\s*$/gm, (_all, heading: string) => /[.!?:;。！？：；]$/.test(heading) ? heading : `${heading}${/[\u3000-\u9fff]$/.test(heading) ? "。" : "."}`)
    .replace(/^\s{0,3}(?:[-*+]\s+|\d+\.\s+)/gm, "")
    .replace(/[`*_>#~]/g, "")
    .replace(/<[^>\n]*>/g, " ")
    .replace(/\s+/g, " ")
    .replace(/([。！？；：]) /g, "$1")
    .trim()
    .slice(0, 240);
}

export function ReferencePreview({
  request,
  onDismiss,
  onOpenPage,
  onOpenMemory,
  onOpen,
  onOpenCitationTarget,
  onPointerEnter,
  onPointerLeave,
  onEscape,
  role = "dialog",
  dataPageLinkPreview = false,
  dataCitationPopover = false,
  id,
  copyPrefix = "reference",
  externalOpenFailure = null,
}: ReferencePreviewProps) {
  const { t, i18n } = useTranslation();
  // Same rule as the page's info panel: English keeps the short "15h ago",
  // Chinese gets its own locale so English never leaks into the card.
  const relativeLocale = i18n.language?.startsWith("zh") ? i18n.language : undefined;
  const boxRef = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(false);
  const [openFailure, setOpenFailure] = useState<{ key: string; anchor: HTMLElement | null; message: string } | null>(null);
  const mounted = useRef(true);
  const target = request?.target;
  const requestKey = !request ? "" : target?.kind === "citation"
    ? `citation:${target.citation.occurrence}:${target.citation.source_kind}:${target.citation.locator}`
    : `${target?.kind}:${target?.id}`;
  const currentRequestKey = useRef(requestKey);
  currentRequestKey.current = requestKey;
  const currentAnchor = request?.anchor ?? null;
  const currentAnchorRef = useRef(currentAnchor);
  currentAnchorRef.current = currentAnchor;
  const pageId = target?.kind === "page" ? target.id : "";
  const memoryId = target?.kind === "memory" ? target.id : "";
  const pageQuery = useQuery({
    queryKey: ["reference-preview", "page", pageId],
    queryFn: () => getPage(pageId),
    enabled: !!request && ready && !!pageId,
    staleTime: 30_000,
    retry: false,
  });
  const memoryQuery = useQuery({
    queryKey: ["reference-preview", "memory", memoryId],
    queryFn: () => getMemoryDetail(memoryId),
    enabled: !!request && ready && !!memoryId,
    staleTime: 30_000,
    retry: false,
  });

  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => { setOpenFailure(null); }, [requestKey, currentAnchor]);

  useEffect(() => {
    setReady(false);
    if (!request) return;
    if (request.target.kind === "citation") {
      setReady(true);
      return;
    }
    const timer = window.setTimeout(() => setReady(true), 180);
    return () => window.clearTimeout(timer);
  }, [request?.target.kind, pageId, memoryId, request?.anchor]);

  const position = useReferencePopoverPosition(ready ? request?.anchor ?? null : null, boxRef, WIDTH);

  useEffect(() => {
    if (!request) return;
    const checkAnchor = () => { if (!request.anchor.isConnected) onDismiss(); };
    const timer = window.setInterval(checkAnchor, 250);
    return () => window.clearInterval(timer);
  }, [request, onDismiss]);

  useReferencePreviewKeyboard({
    anchor: request?.anchor ?? null,
    boxRef,
    enabled: ready && !!request,
    keyboardEntry: !!request?.keyboard,
    onDismiss,
    onEscape,
  });

  if (!request || !ready || typeof document === "undefined" || !document.body) return null;
  const page: Page | null | undefined = target?.kind === "page" ? pageQuery.data : undefined;
  const citationTarget = target?.kind === "citation" ? target : null;
  const memory: MemoryItem | null | undefined = target?.kind === "memory" ? memoryQuery.data : citationTarget?.sourceMemory;
  const anchorLabel = request.anchor.dataset.referenceTargetLabel || request.anchor.dataset.wikiTargetLabel || request.anchor.textContent?.trim() || "";
  const citation = citationTarget?.citation ?? null;
  const kind: ReferenceKind = citation ? citationReferenceKind(citation) : target?.kind === "memory" ? "memory" : "page";
  const app = citation ? citationSourceApp(citation) : null;
  const appName = app ? sourceAppName(app, i18n.language) : null;
  const typeLabel = appName ?? t(`reference.type.${kind}`);
  const fallbackCitationLabel = citation ? appName ?? citationDisplayLabel(citation, {
    memory: t("citation.kind.memory"), external_file: t("citation.kind.file"), external_url: t("citation.kind.url"), authored: t("citation.kind.authored"),
  }) : "";
  // The card's header already names the kind, so an external source's title is just its file or host name.
  const citationTitle = citation ? citationExternalDetail(citation) ?? fallbackCitationLabel : "";
  const memoryCitationGone = citation?.source_kind === "memory" && !memory && !citationTarget?.sourcesLoading;
  const title = page?.title ?? memory?.title ?? (memoryCitationGone ? t("citation.missingTitle") : citation ? citationTitle : anchorLabel);
  const citationLocation = citation?.source_kind === "external_file" ? citationFilePath(citation.locator) : citation?.locator;
  const query = pageId ? pageQuery : memoryId ? memoryQuery : null;
  const content = page?.summary || (page ? page.content : memory?.summary || memory?.content) || "";
  const excerpt = content ? referenceExcerpt(content) : "";
  const citationMissing = !!citation && citation.source_kind !== "authored" && !memory && !citationTarget?.sourcesLoading;
  const loading = !!query?.isPending || !!(citationTarget?.sourcesLoading && !memory);
  const error = !!query?.isError;
  const unavailable = !loading && !error && ((pageId && !page) || (memoryId && !memory) || citationMissing || (!!citation && citation.source_kind !== "authored" && !!memory && !content));

  const open = async () => {
    if (target?.kind === "page") {
      if (onOpen) onOpen(target); else onOpenPage?.(target.id);
    } else if (target?.kind === "memory") {
      if (onOpen) onOpen(target); else onOpenMemory?.(target.id);
    } else if (citation?.source_kind === "memory") {
      const memoryTarget = { kind: "memory", id: citation.locator } as const;
      if (onOpen) onOpen(memoryTarget); else onOpenMemory?.(citation.locator);
    } else if (citation && (citation.source_kind === "external_file" || citation.source_kind === "external_url")) {
      if (onOpenCitationTarget) onOpenCitationTarget(citation);
      else {
        const failure = await openCitationTarget(citation);
        if (mounted.current && currentRequestKey.current === requestKey && currentAnchorRef.current === request?.anchor) {
          setOpenFailure(failure ? { key: requestKey, anchor: request?.anchor ?? null, message: failure } : null);
        }
      }
    }
  };
  const canOpen = target?.kind === "page" || target?.kind === "memory" || (citation?.source_kind === "memory" && !!memory) || citation?.source_kind === "external_file" || citation?.source_kind === "external_url";
  const action = citation?.source_kind === "memory"
    ? t("citation.openMemory")
    : target?.kind === "page" || target?.kind === "memory"
    ? t(`${copyPrefix}.open`, { title })
    : citation?.source_kind === "external_file" ? t("citation.openFile") : citation?.source_kind === "external_url" ? (appName ? t("citation.openInApp", { app: appName }) : t("citation.openLink")) : "";
  const modifiedAgo = citationTarget?.sourceMemory?.last_modified ? relativeMs(citationTarget.sourceMemory.last_modified * 1000, relativeLocale) : "";
  const activeOpenFailure = openFailure?.key === requestKey && openFailure.anchor === request.anchor ? openFailure.message : null;
  const preview = (
    <div
      ref={boxRef}
      data-reference-preview="true"
      data-reference-owner={request.anchor.closest('[role="dialog"], [role="complementary"]')?.getAttribute("aria-labelledby") ?? undefined}
      data-page-link-preview={dataPageLinkPreview ? "true" : undefined}
      data-citation-popover={dataCitationPopover ? "true" : undefined}
      data-missing-source={memoryCitationGone ? citation?.locator : undefined}
      id={id}
      role={role}
      aria-label={title}
      onPointerEnter={onPointerEnter}
      onPointerLeave={onPointerLeave}
      onFocusCapture={onPointerEnter}
      onBlurCapture={(event) => {
        const related = event.relatedTarget;
        if (related instanceof Node && (event.currentTarget.contains(related) || request.anchor.contains(related))) return;
        onPointerLeave();
      }}
      className="reference-preview flex flex-col gap-2 rounded-lg p-3"
      style={{
        fontStyle: "normal", position: "fixed", top: position.top, left: position.left, width: `${Math.min(WIDTH, Math.max(0, window.innerWidth - 16))}px`,
        maxHeight: "40vh", overflow: "auto", zIndex: position.zIndex, boxSizing: "border-box",
        background: "var(--mem-surface)", color: "var(--mem-text)", border: "1px solid var(--mem-border)",
        boxShadow: "var(--mem-popover-shadow)", fontFamily: "var(--mem-font-body)",
      }}
    >
      <div className="flex items-center gap-1.5" style={{ color: "var(--mem-text-tertiary)", fontSize: "var(--mem-text-xs)" }}>
        <ReferenceTypeIcon kind={kind} size={12} locator={citation?.locator} />
        <span>{typeLabel}</span>
      </div>
      <strong style={{ fontSize: "var(--mem-text-sm)", overflowWrap: "anywhere" }}>{title}</strong>
      {citation?.status === "unverified" && <span style={{ color: "var(--mem-accent-amber)", fontSize: "var(--mem-text-xs)" }}>{t("citation.unverified")}</span>}
      {citation?.source_kind === "authored" && <p style={{ margin: 0, color: "var(--mem-text-secondary)", fontSize: "var(--mem-text-xs)" }}>{t("citation.authoredDescription")}</p>}
      {(activeOpenFailure || externalOpenFailure) && <div role="alert" style={{ color: "var(--mem-accent-amber)", fontSize: "var(--mem-text-xs)" }}>
        <p style={{ margin: 0 }}>{t(citation?.source_kind === "external_file" ? "citation.openFileFailed" : "citation.openLinkFailed")}</p>
        <p style={{ margin: 0, overflowWrap: "anywhere" }}>{activeOpenFailure || externalOpenFailure}</p>
      </div>}
      {loading && <p role="status" style={{ margin: 0, color: "var(--mem-text-secondary)", fontSize: "var(--mem-text-xs)" }}>{t(`${copyPrefix}.loading`)}</p>}
      {loading && citation?.source_kind === "memory" && <div data-testid="citation-popover-skeleton" className="flex flex-col gap-1.5">
        <div style={{ width: "70%", height: "10px", background: "var(--mem-hover)", borderRadius: "4px" }} />
        <div style={{ width: "90%", height: "10px", background: "var(--mem-hover)", borderRadius: "4px" }} />
      </div>}
      {error && <div role="alert" style={{ color: "var(--mem-accent-amber)", fontSize: "var(--mem-text-xs)" }}>
        <span>{t(`${copyPrefix}.error`)}</span>{" "}
        <button type="button" onClick={() => void query?.refetch()} style={{ color: "var(--mem-accent-indigo)" }}>{t(`${copyPrefix}.retry`)}</button>
      </div>}
      {unavailable && <p role="status" style={{ margin: 0, color: "var(--mem-text-secondary)", fontSize: "var(--mem-text-xs)" }}>{citation?.source_kind === "memory" ? t("citation.missingMemory") : t(`${copyPrefix}.unavailable`)}</p>}
      {/* Three lines are enough to recognise the source; the card is for a glance, not for reading. */}
      {excerpt && <p data-reference-excerpt="true" style={{ margin: 0, color: "var(--mem-text-secondary)", fontSize: "var(--mem-text-xs)", lineHeight: 1.5, overflowWrap: "anywhere", display: "-webkit-box", WebkitBoxOrient: "vertical", WebkitLineClamp: 3, overflow: "hidden" }}>{excerpt}</p>}
      {citation && citation.source_kind !== "memory" && citation.source_kind !== "authored" && <p title={citationLocation} style={{ margin: 0, color: "var(--mem-text-tertiary)", fontSize: "10px", whiteSpace: "nowrap", overflow: "hidden", textOverflow: "ellipsis" }}>{citationLocation}</p>}
      {modifiedAgo && !canOpen && <p style={{ margin: 0, color: "var(--mem-text-tertiary)", fontSize: "10px" }}>{modifiedAgo}</p>}
      {/* Pinned to the card's bottom edge so a long excerpt or link never scrolls the open action out of view. */}
      {/* The age shares the action's row so the card ends on one line instead of a stray link under a gap. */}
      {canOpen && <div data-reference-preview-actions="true" className="flex items-baseline justify-between gap-3" style={{ position: "sticky", bottom: "-12px", margin: "0 -12px -12px", padding: "2px 12px 12px", background: "var(--mem-surface)" }}>
        <span style={{ color: "var(--mem-text-tertiary)", fontSize: "10px" }}>{modifiedAgo}</span>
        <button type="button" onClick={open} style={{ color: "var(--mem-accent-indigo)", background: "none", border: 0, padding: 0, cursor: "pointer", font: "inherit", fontSize: "var(--mem-text-xs)", whiteSpace: "nowrap", flexShrink: 0 }}>{action}</button>
      </div>}
    </div>
  );
  return createPortal(preview, document.body);
}
