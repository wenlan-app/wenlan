// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageRevisions, type PageHistoryItem } from "../../../lib/tauri";
import { diffWords } from "../../../lib/wordDiff";
import { DiffText } from "../ReviewDialog";
import { Button } from "../settings/primitives";
import { historyItemLabel } from "./historyItems";
import "./BasisDiffDialog.css";

interface HistoryChangeDialogProps {
  pageId: string;
  /** The history entry the person opened. Matched by kind, version and source. */
  item: PageHistoryItem;
  onClose: () => void;
}

function sameItem(a: PageHistoryItem, b: PageHistoryItem): boolean {
  return a.kind === b.kind
    && a.at === b.at
    && (a.version ?? null) === (b.version ?? null)
    && (a.source?.page_id ?? null) === (b.source?.page_id ?? null)
    && (a.source?.version ?? null) === (b.source?.version ?? null);
}

/**
 * The text before and after one change. An edit compares with the edit just
 * before it; a source's new version compares with that source's version
 * before it in the same history. `before` is null when no earlier text is
 * kept.
 */
export function historyTexts(
  history: readonly PageHistoryItem[],
  item: PageHistoryItem,
): { before: string | null; after: string | null } {
  const index = history.findIndex((candidate) => sameItem(candidate, item));
  const found = index >= 0 ? history[index] : item;
  // Newest first, so what came before sits later in the list.
  const older = index >= 0 ? history.slice(index + 1) : [];
  if (found.kind === "source_update") {
    const previous = older.find((candidate) =>
      candidate.kind === "source_update" && candidate.source?.page_id === found.source?.page_id);
    return { before: previous?.source?.text ?? null, after: found.source?.text ?? null };
  }
  const previous = older.find((candidate) => candidate.kind !== "source_update" && candidate.text != null);
  return { before: previous?.text ?? null, after: found.text ?? null };
}

/** What one entry of the page's history changed (docs/ux/spec.md, UX-HIST-2). */
export function HistoryChangeDialog({ pageId, item, onClose }: HistoryChangeDialogProps) {
  const { t, i18n } = useTranslation();
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const textsQuery = useQuery({
    queryKey: ["page-revisions-text", pageId],
    queryFn: () => getPageRevisions(pageId, true),
    staleTime: 30_000,
    retry: false,
  });

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null;
    panelRef.current?.focus();
    return () => previous?.focus?.();
  }, []);

  const texts = useMemo(
    () => (textsQuery.data ? historyTexts(textsQuery.data.history ?? [], item) : null),
    [textsQuery.data, item],
  );
  const segments = useMemo(
    () => (texts?.after != null && texts.before != null ? diffWords(texts.before, texts.after) : []),
    [texts],
  );

  let body;
  if (textsQuery.isPending) {
    body = <p className="basis-diff__status" role="status">{t("pageHistory.loading")}</p>;
  } else if (textsQuery.isError) {
    body = <p className="basis-diff__status" role="alert">{t("pageHistory.error")}</p>;
  } else if (texts?.after == null) {
    body = <p className="basis-diff__status" role="status">{t("pageHistory.notKept")}</p>;
  } else if (texts.before == null) {
    body = <>
      <p className="basis-diff__status">{t(item.version === 1 ? "pageHistory.firstVersion" : "pageHistory.noEarlier")}</p>
      <div className="basis-diff__text" data-testid="history-change-text">{texts.after}</div>
    </>;
  } else {
    const changed = segments.some((segment) => segment.kind !== "same");
    body = <div className="basis-diff__text" data-testid="history-change-text">
      {changed ? <DiffText segments={segments} /> : t("pageHistory.noTextChange")}
    </div>;
  }

  let when = "";
  try {
    when = new Date(item.at * 1000).toLocaleString(i18n.language, { dateStyle: "medium", timeStyle: "short" });
  } catch {
    when = new Date(item.at * 1000).toISOString();
  }

  return createPortal(
    <div
      className="basis-diff-backdrop"
      onClick={(event) => {
        if (event.target === event.currentTarget) onClose();
      }}
    >
      <div
        ref={panelRef}
        tabIndex={-1}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        className="basis-diff-panel"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onClose();
          }
        }}
      >
        <h2 id={titleId} className="basis-diff__title">{historyItemLabel(t, item)}</h2>
        <p className="basis-diff__versions">{when}</p>
        {body}
        <div className="basis-diff__actions">
          <Button variant="secondary" onClick={onClose}>{t("pageHistory.close")}</Button>
        </div>
      </div>
    </div>,
    document.body,
  );
}

export default HistoryChangeDialog;
