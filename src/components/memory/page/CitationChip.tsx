// SPDX-License-Identifier: AGPL-3.0-only
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { MemoryItem, PageCitation } from "../../../lib/tauri";
import {
  citationDisplayLabel,
  type CitationKindLabels,
} from "../../../lib/pageCitations";
import CitationPopover, { openCitationTarget } from "./CitationPopover";
import { ReferenceTypeIcon, citationReferenceKind, citationSourceApp } from "../links/ReferenceTypeIcon";
import { sourceAppName } from "../sources/sourcePresentation";
import "../links/reference-links.css";

interface CitationChipProps {
  /** The source's footnote number: a source cited again shows the number of
   *  its first citation, not its position among the page's markers. */
  number: number;
  citation: PageCitation;
  sourceMemory: MemoryItem | null;
  sourcesLoading: boolean;
  onOpenMemory: (sourceId: string) => void;
}

const HOVER_OPEN_DELAY_MS = 150;
const HOVER_CLOSE_GRACE_MS = 120;

export default function CitationChip({
  number,
  citation,
  sourceMemory,
  sourcesLoading,
  onOpenMemory,
}: CitationChipProps) {
  const [open, setOpen] = useState(false);
  const [openFailure, setOpenFailure] = useState<string | null>(null);
  const { t, i18n } = useTranslation();
  const chipRef = useRef<HTMLButtonElement>(null);
  const openTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const lastPointerType = useRef("mouse");
  const popoverId = useId();
  const kindLabels: CitationKindLabels = {
    memory: t("citation.kind.memory"),
    external_file: t("citation.kind.file"),
    external_url: t("citation.kind.url"),
    authored: t("citation.kind.authored"),
  };
  const app = citationSourceApp(citation);
  const displayLabel = app ? sourceAppName(app, i18n.language) : citationDisplayLabel(citation, kindLabels);

  const clearCloseTimer = () => {
    if (closeTimer.current) clearTimeout(closeTimer.current);
  };
  const scheduleClose = () => {
    if (openTimer.current) clearTimeout(openTimer.current);
    clearCloseTimer();
    closeTimer.current = setTimeout(() => setOpen(false), HOVER_CLOSE_GRACE_MS);
  };
  const handleMouseEnter = () => {
    clearCloseTimer();
    openTimer.current = setTimeout(() => setOpen(true), HOVER_OPEN_DELAY_MS);
  };
  const handlePopoverMouseEnter = () => {
    clearCloseTimer();
  };
  const isPopoverTarget = (target: EventTarget | null) => {
    if (!(target instanceof Node)) return false;
    return document.getElementById(popoverId)?.contains(target) ?? false;
  };
  const focusFirstPopoverControl = () => {
    const popover = document.getElementById(popoverId);
    const firstControl = popover?.querySelector<HTMLElement>(
      "button:not([disabled]), summary, a[href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    );
    if (!firstControl) return false;
    firstControl.focus();
    return true;
  };

  const clearTimers = () => {
    if (openTimer.current) clearTimeout(openTimer.current);
    clearCloseTimer();
  };
  const closePopover = () => {
    clearTimers();
    // Restoring focus fires onFocus synchronously; close after that event.
    chipRef.current?.focus();
    setOpen(false);
  };
  useEffect(() => clearTimers, []);
  // A refusal is about the click that caused it, not the next hover.
  useEffect(() => {
    if (!open) setOpenFailure(null);
  }, [open]);

  // Never resolves to a rejection: a refused open is shown in the popover,
  // which is opened if it was not already (a chip click on a link skips it).
  const openTarget = async () => {
    setOpenFailure(null);
    const failure = await openCitationTarget(citation);
    if (failure !== null) {
      setOpenFailure(failure);
      setOpen(true);
    }
  };

  const activate = () => {
    // Touch has no hover: first tap opens the popover, its buttons navigate.
    if (lastPointerType.current === "touch") {
      lastPointerType.current = "mouse";
      clearTimers();
      setOpen(true);
      return;
    }
    if (citation.source_kind === "memory") {
      // A deleted/merged source has no detail view to land on — explain in
      // the popover instead of navigating to a blank page.
      if (!sourceMemory && !sourcesLoading) {
        setOpen((v) => !v);
        return;
      }
      onOpenMemory(citation.locator);
    } else if (citation.source_kind === "external_url") {
      void openTarget();
    } else {
      setOpen((v) => !v);
    }
  };

  const unverified = citation.status === "unverified";

  return (
    <span
      style={{ position: "relative", display: "inline-block" }}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={scheduleClose}
      onFocus={() => setOpen(true)}
      onBlur={(e) => {
        // Keep open while focus moves into the popover (its action button).
        if (
          !e.currentTarget.contains(e.relatedTarget as Node | null) &&
          !isPopoverTarget(e.relatedTarget)
        ) {
          setOpen(false);
        }
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          closePopover();
        }
        if (e.key === "Tab" && !e.shiftKey && e.target === chipRef.current) {
          if (focusFirstPopoverControl()) e.preventDefault();
        }
      }}
    >
      <button
        ref={chipRef}
        type="button"
        data-status={citation.status}
        data-reference-kind={citationReferenceKind(citation)}
        aria-describedby={open ? popoverId : undefined}
        onPointerDown={(e) => {
          lastPointerType.current = e.pointerType;
        }}
        onClick={activate}
        className="reference-link reference-link--source citation-chip focus-visible:outline-2 focus-visible:outline-[var(--mem-accent-indigo)]"
        style={{
          fontFamily: "var(--mem-font-body)",
          // A footnote step below the reading size: 12px beside 15px body text.
          fontSize: "calc(var(--content-body-size, 15px) * 0.8)",
          lineHeight: 1,
          border: unverified ? "1px dashed var(--mem-accent-amber)" : "0",
          borderRadius: "3px",
          padding: "1px 2px",
          margin: "0 2px",
          verticalAlign: "baseline",
          cursor: "pointer",
          maxWidth: "min(180px, 28vw)",
          overflow: "hidden",
          whiteSpace: "nowrap",
          textOverflow: "ellipsis",
          textDecoration: "underline",
          textUnderlineOffset: "2px",
          display: "inline-flex",
          alignItems: "baseline",
          gap: "2px",
        }}
      >
        <ReferenceTypeIcon kind={citationReferenceKind(citation)} size={12} locator={citation.locator} />
        <span
          style={{
            display: "inline-block",
            maxWidth: "150px",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
            verticalAlign: "bottom",
          }}
        >
          {displayLabel}
        </span>
        <sup style={{ marginLeft: "1px" }}>{number}</sup>
      </button>
      {open && (
        <CitationPopover
          id={popoverId}
          citation={citation}
          kindLabels={kindLabels}
          sourceMemory={sourceMemory}
          sourcesLoading={sourcesLoading}
          anchorRef={chipRef}
          onOpenMemory={onOpenMemory}
          openFailure={openFailure}
          onOpenTarget={() => void openTarget()}
          onMouseEnter={handlePopoverMouseEnter}
          onMouseLeave={scheduleClose}
          onEscape={closePopover}
        />
      )}
    </span>
  );
}
