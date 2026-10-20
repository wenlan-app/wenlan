// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useMemo, useRef } from "react";
import { createPortal } from "react-dom";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getPageBasisDiff, type BasisDiff } from "../../../lib/tauri";
import { diffWords } from "../../../lib/wordDiff";
import { DiffText } from "../ReviewDialog";
import { Button } from "../settings/primitives";
import { revisionWriterLabel } from "./format";
import "./BasisDiffDialog.css";

interface BasisDiffDialogProps {
  pageId: string;
  basisPageId: string;
  /** The basis note's title for the header line. */
  basisTitle: string;
  /** Confirm "no impact" at the version this dialog showed. */
  onConfirm: (version: number) => void;
  onClose: () => void;
  confirming?: boolean;
}

// The writers the daemon records for a person (app editor, vault file edit).
// Anything else is described by the shared revision label, never the raw slug.
const HUMAN_WRITERS = new Set(["manual_edit", "fs_edit", "human"]);

function whenLabel(seconds: number, language: string): string {
  try {
    return new Date(seconds * 1000).toLocaleString(language, { dateStyle: "medium", timeStyle: "short" });
  } catch {
    return new Date(seconds * 1000).toISOString();
  }
}

/** What changed in a note this one relies on, since the person last checked. */
export function BasisDiffDialog({
  pageId,
  basisPageId,
  basisTitle,
  onConfirm,
  onClose,
  confirming = false,
}: BasisDiffDialogProps) {
  const { t, i18n } = useTranslation();
  const titleId = useId();
  const panelRef = useRef<HTMLDivElement>(null);
  const diffQuery = useQuery({
    queryKey: ["page-basis-diff", pageId, basisPageId],
    queryFn: () => getPageBasisDiff(pageId, basisPageId),
    // The answer is only good for the moment it is read: always ask again.
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

  const diff: BasisDiff | null = diffQuery.data?.outcome === "available" ? diffQuery.data.diff : null;
  const segments = useMemo(
    () => (diff ? diffWords(diff.from_content, diff.to_content) : []),
    [diff],
  );
  const textChanged = segments.some((segment) => segment.kind !== "same");
  const title = basisTitle || t("noteBasis.untitled");

  let body;
  if (diffQuery.isPending) {
    body = <p className="basis-diff__status" role="status">{t("noteBasis.diff.loading")}</p>;
  } else if (diffQuery.isError) {
    body = <p className="basis-diff__status" role="alert">{t("noteBasis.diff.error")}</p>;
  } else if (!diff) {
    body = <p className="basis-diff__status" role="status">{t("noteBasis.diff.unavailable")}</p>;
  } else {
    body = <>
      <p className="basis-diff__versions">
        {t("noteBasis.diff.versions", { title, from: diff.from_version, to: diff.to_version })}
      </p>
      <div className="basis-diff__text" data-testid="basis-diff-text">
        {textChanged ? <DiffText segments={segments} /> : t("noteBasis.diff.noTextChange")}
      </div>
      {diff.edits.length > 0 && <section aria-label={t("noteBasis.diff.edits")}>
        <h3 className="basis-diff__heading">{t("noteBasis.diff.edits")}</h3>
        <ul className="basis-diff__edits">
          {diff.edits.map((edit) => <li key={edit.version}>
            <span className="basis-diff__edit-meta">
              {HUMAN_WRITERS.has(edit.edited_by) ? t("noteBasis.diff.you") : revisionWriterLabel(t, edit.edited_by)} · {whenLabel(edit.at, i18n.language)}
              {edit.minor && <span className="basis-diff__minor">{t("noteBasis.diff.minorEdit")}</span>}
            </span>
          </li>)}
        </ul>
      </section>}
    </>;
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
            return;
          }
          if (event.key !== "Tab") return;
          const controls = Array.from(
            panelRef.current?.querySelectorAll<HTMLElement>("button:not(:disabled)") ?? [],
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
        <h2 id={titleId} className="basis-diff__title">{t("noteBasis.diff.title")}</h2>
        {body}
        <div className="basis-diff__actions">
          <Button variant="secondary" onClick={onClose}>{t("noteBasis.diff.close")}</Button>
          {diff && <Button
            variant="primary"
            loading={confirming}
            onClick={() => onConfirm(diff.to_version)}
          >{t("noteBasis.confirmNoImpact")}</Button>}
        </div>
      </div>
    </div>,
    document.body,
  );
}

export default BasisDiffDialog;
