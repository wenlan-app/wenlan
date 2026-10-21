// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef } from "react";
import { createPortal } from "react-dom";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageCodeBasisCited, readCodeLines, readGitBlobLines, type CodeBasisRef } from "../../../lib/tauri";
import { Button } from "../settings/primitives";
import {
  codeBasisAbsolutePath,
  codeBasisLocation,
  codeBasisSpanVars,
  useCopyLocation,
} from "./codeBasisShared";
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
}

function Column({ title, children }: { title: string; children: React.ReactNode }) {
  return <section className="code-basis__column">
    <h3 className="code-basis__column-title">{title}</h3>
    {children}
  </section>;
}

function Code({ lines, firstLine, testId }: { lines: readonly string[]; firstLine: number; testId: string }) {
  return <pre className="code-basis__code" data-testid={testId} tabIndex={0}>
    {lines.map((line, index) => <span key={index} className="code-basis__line">
      <span className="code-basis__number" aria-hidden="true">{firstLine + index}</span>
      <span>{line}</span>
    </span>)}
  </pre>;
}

/**
 * The lines a note relies on, side by side: as they read when the note cited
 * them, and as they read now. A plain comparison; nothing is coloured.
 */
export function CodeBasisDialog({ pageId, row, onConfirm, onClose, confirming = false }: CodeBasisDialogProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const { copied, copy } = useCopyLocation();
  const rowKey = codeBasisKeyOf(pageId, row);
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
  });
  const now = useQuery({
    queryKey: ["code-basis-now", rowKey],
    queryFn: () => readCodeLines(codeBasisAbsolutePath(row), row.line_start, row.line_end),
    staleTime: 0,
    gcTime: 0,
    retry: false,
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

  let before;
  if (cited.isPending) {
    before = <p className="basis-diff__status" role="status">{t("codeBasis.dialog.loading")}</p>;
  } else if (cited.isError || cited.data == null) {
    before = <p className="basis-diff__status" role="status">{t("codeBasis.dialog.noBefore")}</p>;
  } else {
    before = <Code lines={cited.data} firstLine={row.line_start} testId="code-basis-before" />;
  }

  let after;
  if (now.isPending) {
    after = <p className="basis-diff__status" role="status">{t("codeBasis.dialog.loading")}</p>;
  } else if (now.isError) {
    after = <p className="basis-diff__status" role="alert">{t("codeBasis.dialog.unavailable")}</p>;
  } else {
    after = <Code lines={now.data} firstLine={row.line_start} testId="code-basis-after" />;
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
            panelRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled), pre[tabindex]") ?? [],
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
        <h2 id={titleId} className="basis-diff__title">{title}</h2>
        <div className="code-basis__columns">
          <Column title={t("codeBasis.dialog.before")}>{before}</Column>
          <Column title={t("codeBasis.dialog.after")}>{after}</Column>
        </div>
        <div className="basis-diff__actions">
          <Button variant="secondary" onClick={() => { void copy(codeBasisLocation(row)); }}>
            {copied ? t("codeBasis.copied") : t("codeBasis.copyLocation")}
          </Button>
          <Button variant="secondary" onClick={onClose}>{t("codeBasis.close")}</Button>
          {row.state === "pending" && <Button
            variant="primary"
            loading={confirming}
            onClick={onConfirm}
          >{t("codeBasis.confirmNoImpact")}</Button>}
        </div>
      </div>
    </div>,
    document.body,
  );
}

export default CodeBasisDialog;
