// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageCodeBasisCited, readCodeLines, readGitBlobLines, type CodeBasisRef } from "../../../lib/tauri";
import { Button } from "../settings/primitives";
import {
  codeBasisAbsolutePath,
  codeBasisLineText,
  codeBasisLocation,
  codeBasisSpanVars,
  useCopyLocation,
} from "./codeBasisShared";
import { alignCodeLines, CURRENT_LINES_EXTRA, diffCodeLines, fitCurrentLines, type CodeRow } from "./codeBasisDiff";
import { codeBasisKeyOf } from "./useCodeBasisActions";
import "./BasisDiffDialog.css";
import "./CodeBasisDialog.css";

interface CodeBasisDialogProps {
  pageId: string;
  row: CodeBasisRef;
  /** Confirm "no impact" for these lines as they read now. */
  onConfirm: () => void;
  onClose: () => void;
  confirming?: boolean;
  /**
   * Present when the person decides here, one row after another (the page's
   * own notice): the step, what the check said, and the other ways out. The
   * review list leaves it out and keeps its copy and close buttons.
   */
  decide?: CodeBasisDecide;
}

export interface CodeBasisDecide {
  step: { index: number; total: number };
  /** What the background check said about the row, as the notice used to show it. */
  note: string | null;
  /** A conflict or a failure from the last try, said in the dialog. */
  message: { text: string; alert: boolean } | null;
  onStop: () => void;
  onEdit: () => void;
  /** Leave a row that cannot be found for now and go on. */
  onLater: () => void;
  stopping?: boolean;
}

function Column({ title, children }: { title: string; children: React.ReactNode }) {
  return <section className="code-basis__column">
    <h3 className="code-basis__column-title">{title}</h3>
    {children}
  </section>;
}

/** The rows to draw: folds the person opened are replaced by the lines they hold. */
function openFolds(rows: readonly CodeRow[], open: ReadonlySet<number>): CodeRow[] {
  return rows.flatMap((row) => (row.kind === "fold" && open.has(row.id) ? row.rows : [row]));
}

/**
 * One side of the code. Without `rows` it is the lines as they are, one per
 * row. With `rows` both sides draw the same rows, so they line up: a blank
 * filler stands in for a line the other side has, and a fold for a run of
 * equal lines, which opens on both sides at once.
 */
function Code({ lines, firstLine, testId, rows, side, onOpenFold, boxRef, onScroll }: {
  lines: readonly string[];
  firstLine: number;
  testId: string;
  rows?: readonly CodeRow[];
  side: "before" | "after";
  onOpenFold?: (id: number) => void;
  boxRef?: React.Ref<HTMLPreElement>;
  onScroll?: React.UIEventHandler<HTMLPreElement>;
}) {
  const { t } = useTranslation();
  const mark = side === "before" ? "removed" : "added";
  const line = (index: number, changed: boolean, key: React.Key) => <span
    key={key}
    className={changed ? `code-basis__line code-basis__line--${mark}` : "code-basis__line"}
    data-change={changed ? mark : undefined}
  >
    <span className="code-basis__number" aria-hidden="true">{firstLine + index}</span>
    <span className="code-basis__text">{lines[index]}</span>
  </span>;
  // One focusable scroller per box: the sideways one. Arrow keys on it scroll
  // sideways and, past its edge, the box up and down.
  return <pre className="code-basis__code" data-testid={testId} ref={boxRef} onScroll={onScroll}>
    <div className="code-basis__scroll" tabIndex={0}>
      <div className="code-basis__lines">
        {rows ? rows.map((row, index) => {
          if (row.kind === "fold") {
            return <button
              key={`fold-${row.id}`}
              type="button"
              // Both sides open the same fold: one keyboard stop is enough.
              tabIndex={side === "before" ? undefined : -1}
              className="code-basis__fold"
              onClick={() => onOpenFold?.(row.id)}
            >{t("codeBasis.dialog.unchanged", { count: row.count })}</button>;
          }
          const cell = side === "before" ? row.before : row.after;
          return cell
            ? line(cell.index, cell.changed, index)
            : <span key={index} className="code-basis__filler" aria-hidden="true">{"\u200b"}</span>;
        }) : lines.map((_, index) => line(index, false, index))}
      </div>
    </div>
  </pre>;
}

/**
 * The lines a note relies on, side by side: as they read when the note cited
 * them, and as they read now. Lines that exist on one side only are tinted
 * (removed on the cited side, added on the current side); equal lines stay plain.
 */
export function CodeBasisDialog({ pageId, row, onConfirm, onClose, confirming = false, decide }: CodeBasisDialogProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const { copied, copy } = useCopyLocation();
  const rowKey = codeBasisKeyOf(pageId, row);
  // Code that cannot be found has nothing to show side by side; the notice's
  // own sentence says why.
  const missing = Boolean(decide) && row.state === "unavailable";
  // The lines as cited: the file's git blob from when the note cited it, read
  // for these lines. No blob (an older row) is "nothing to compare"; a blob
  // that git cannot read is an error, which the view treats the same way.
  const cited = useQuery({
    queryKey: ["code-basis-cited", rowKey],
    queryFn: async (): Promise<string[] | null> => {
      const { cited_blob: blob } = await getPageCodeBasisCited(pageId, row);
      return blob ? readGitBlobLines(row.repo_root, blob, row.line_start, row.line_end) : null;
    },
    // The answer is only good for the moment it is read: always ask again.
    staleTime: 0,
    gcTime: 0,
    retry: false,
    enabled: !missing,
  });
  const now = useQuery({
    queryKey: ["code-basis-now", rowKey],
    // A little past the span: an insertion inside it pushes its last lines down.
    queryFn: () => readCodeLines(codeBasisAbsolutePath(row), row.line_start, row.line_end + CURRENT_LINES_EXTRA),
    staleTime: 0,
    gcTime: 0,
    retry: false,
    enabled: !missing,
  });

  useEffect(() => {
    // Return focus to whatever opened the dialog once it closes.
    const opener = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    panelRef.current?.focus();
    return () => {
      if (opener?.isConnected) opener.focus();
    };
  }, []);

  const vars = codeBasisSpanVars(row);
  const title = t(row.line_start === row.line_end ? "codeBasis.dialog.titleOne" : "codeBasis.dialog.title", vars);

  const citedLines = cited.isError || cited.data == null ? null : cited.data;
  const noBefore = !cited.isPending && citedLines == null;
  const spanLength = row.line_end - row.line_start + 1;
  const nowLines = useMemo(
    () => (now.data && !cited.isPending ? fitCurrentLines(citedLines, now.data, spanLength) : null),
    [now.data, cited.isPending, citedLines, spanLength],
  );
  const diff = useMemo(
    () => (citedLines && nowLines ? diffCodeLines(citedLines, nowLines) : null),
    [citedLines, nowLines],
  );
  const rows = useMemo(() => (diff ? alignCodeLines(diff) : null), [diff]);
  const [openFoldIds, setOpenFoldIds] = useState<ReadonlySet<number>>(new Set());
  const shown = rows ? openFolds(rows, openFoldIds) : undefined;
  const openFold = (id: number) => setOpenFoldIds((current) => new Set(current).add(id));
  // The two boxes scroll up and down together; each still scrolls sideways on its own.
  const beforeBox = useRef<HTMLPreElement>(null);
  const afterBox = useRef<HTMLPreElement>(null);
  const follow = (from: React.RefObject<HTMLPreElement | null>, to: React.RefObject<HTMLPreElement | null>) => () => {
    if (from.current && to.current && to.current.scrollTop !== from.current.scrollTop) {
      to.current.scrollTop = from.current.scrollTop;
    }
  };

  let before;
  if (cited.isPending) {
    before = <p className="basis-diff__status" role="status">{t("codeBasis.dialog.loading")}</p>;
  } else if (citedLines == null) {
    // Nothing to compare: the message goes above the one column, see below.
    before = null;
  } else {
    before = <Code
      lines={citedLines}
      firstLine={row.line_start}
      testId="code-basis-before"
      rows={shown}
      side="before"
      onOpenFold={openFold}
      boxRef={beforeBox}
      onScroll={follow(beforeBox, afterBox)}
    />;
  }

  let after;
  if (now.isPending || (!now.isError && nowLines === null)) {
    after = <p className="basis-diff__status" role="status">{t("codeBasis.dialog.loading")}</p>;
  } else if (now.isError) {
    after = <p className="basis-diff__status" role="alert">{t("codeBasis.dialog.unavailable")}</p>;
  } else {
    after = <Code
      lines={nowLines ?? []}
      firstLine={row.line_start}
      testId="code-basis-after"
      rows={shown}
      side="after"
      onOpenFold={openFold}
      boxRef={afterBox}
      onScroll={follow(afterBox, beforeBox)}
    />;
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
        className="basis-diff-panel code-basis-panel"
        onKeyDown={(event) => {
          if (event.key === "Escape") {
            event.preventDefault();
            event.stopPropagation();
            onClose();
            return;
          }
          if (event.key !== "Tab") return;
          const controls = Array.from(
            panelRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled), .code-basis__scroll[tabindex]") ?? [],
          );
          const first = controls[0];
          const last = controls[controls.length - 1];
          if (event.shiftKey && (document.activeElement === first || document.activeElement === panelRef.current)) {
            event.preventDefault();
            last?.focus();
          } else if (!event.shiftKey && document.activeElement === last) {
            event.preventDefault();
            first?.focus();
          }
        }}
      >
        {decide ? <div className="code-basis__head">
          <h2 id={titleId} className="basis-diff__title">{title}</h2>
          {decide.step.total > 1 && <span className="code-basis__step">
            {t("codeBasis.step", { index: decide.step.index + 1, total: decide.step.total })}
          </span>}
          <Button variant="ghost" size="sm" aria-label={t("codeBasis.close")} onClick={onClose}>×</Button>
        </div> : <h2 id={titleId} className="basis-diff__title">{title}</h2>}
        {decide?.note && <p className="basis-diff__status">{decide.note}</p>}
        {missing && <p className="basis-diff__status">{codeBasisLineText(t, row)}</p>}
        {!missing && noBefore && <p className="basis-diff__status" role="status">{t("codeBasis.dialog.noBefore")}</p>}
        {diff?.tooLarge && <p className="basis-diff__status" role="status">{t("codeBasis.dialog.tooLong")}</p>}
        {!missing && <div className={noBefore ? "code-basis__columns code-basis__columns--single" : "code-basis__columns"}>
          {!noBefore && <Column title={t("codeBasis.dialog.before")}>{before}</Column>}
          <Column title={t("codeBasis.dialog.after")}>{after}</Column>
        </div>}
        {decide?.message && <p className="basis-diff__status" role={decide.message.alert ? "alert" : "status"}>
          {decide.message.text}
        </p>}
        {decide ? <div className="basis-diff__actions code-basis__decide">
          <Button variant="ghost" onClick={decide.onEdit}>{t("codeBasis.editYourself")}</Button>
          <div className="code-basis__decide-main">
            {row.state === "pending" ? <>
              <Button variant="secondary" loading={decide.stopping} disabled={confirming} onClick={decide.onStop}>
                {t("codeBasis.stopRelying")}
              </Button>
              <Button variant="primary" loading={confirming} disabled={decide.stopping} onClick={onConfirm}>
                {t("codeBasis.confirmNoImpact")}
              </Button>
            </> : <>
              <Button variant="secondary" disabled={decide.stopping} onClick={decide.onLater}>{t("codeBasis.later")}</Button>
              <Button variant="primary" loading={decide.stopping} onClick={decide.onStop}>{t("codeBasis.stopRelying")}</Button>
            </>}
          </div>
        </div> : <div className="basis-diff__actions">
          <Button variant="secondary" onClick={() => { void copy(codeBasisLocation(row)); }}>
            {copied ? t("codeBasis.copied") : t("codeBasis.copyLocation")}
          </Button>
          <Button variant="secondary" onClick={onClose}>{t("codeBasis.close")}</Button>
          {row.state === "pending" && <Button
            variant="primary"
            loading={confirming}
            onClick={onConfirm}
          >{t("codeBasis.confirmNoImpact")}</Button>}
        </div>}
      </div>
    </div>,
    document.body,
  );
}

export default CodeBasisDialog;
