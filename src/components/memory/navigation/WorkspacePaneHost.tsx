// SPDX-License-Identifier: AGPL-3.0-only
import { createContext, type RefObject } from "react";

/** The note group whose local workspace hosts own inspector panes. */
export interface WorkspaceNoteGroupContextValue {
  readonly id: string;
  readonly element: HTMLElement | null;
}

export const WorkspaceNoteGroupContext = createContext<WorkspaceNoteGroupContextValue | null>(null);

/** The optional right-side workspace column reserved for nonmodal panes. */
export const WorkspacePaneHostContext = createContext<HTMLElement | null>(null);

/** Optional titlebar slot for the active document’s context toggle. */
export const WorkspaceDocumentToolsHostContext = createContext<HTMLElement | null>(null);

/** A single contextual inspector shared by the split Wiki reading workspace. */
export interface SharedWikiInspectorContextValue {
  readonly activeGroupId: string;
  readonly readingElement: HTMLElement | null;
  readonly toolsElement: HTMLElement | null;
  /** Whether the Wiki reading area currently shows two side-by-side groups. */
  readonly isSplit?: boolean;
}

export const SharedWikiInspectorContext = createContext<SharedWikiInspectorContextValue | null>(null);

/** Main reading area for expanding a contextual source without losing its parent. */
export const WorkspaceReadingHostContext = createContext<RefObject<HTMLElement | null> | null>(null);
