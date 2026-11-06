// SPDX-License-Identifier: AGPL-3.0-only
import type { MemoryItem, PageCitation } from "../../../lib/tauri";
import type { CitationKindLabels } from "../../../lib/pageCitations";
import { ReferencePreview } from "../links/ReferencePreview";
import { citationFilePath, openCitationTarget } from "../links/openCitationTarget";

interface CitationPopoverProps {
  id: string;
  citation: PageCitation;
  kindLabels: CitationKindLabels;
  sourceMemory: MemoryItem | null;
  sourcesLoading: boolean;
  anchorRef: React.RefObject<HTMLButtonElement | null>;
  onOpenMemory: (sourceId: string, citation?: PageCitation) => void;
  /** Why the last attempt to open this citation's file or link was refused. */
  openFailure: string | null;
  onOpenTarget: () => void;
  onMouseEnter: () => void;
  onMouseLeave: () => void;
  onEscape: () => void;
}

export default function CitationPopover({
  id,
  citation,
  sourceMemory,
  sourcesLoading,
  anchorRef,
  onOpenMemory,
  openFailure,
  onMouseEnter,
  onMouseLeave,
  onEscape,
}: CitationPopoverProps) {
  const anchor = anchorRef.current;
  if (!anchor) return null;
  return (
    <ReferencePreview
      request={{ target: { kind: "citation", citation, sourceMemory, sourcesLoading }, anchor, keyboard: true }}
      id={id}
      role="tooltip"
      dataCitationPopover
      externalOpenFailure={openFailure}
      onDismiss={onEscape}
      onOpenMemory={onOpenMemory}
      onPointerEnter={onMouseEnter}
      onPointerLeave={onMouseLeave}
      onEscape={onEscape}
    />
  );
}

export { citationFilePath, openCitationTarget };
