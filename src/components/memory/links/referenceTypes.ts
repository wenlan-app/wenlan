// SPDX-License-Identifier: AGPL-3.0-only
import type { MemoryItem, PageCitation } from "../../../lib/tauri";

export type ReferenceTarget =
  /** `heading`: the section a `[[Page#Heading]]` link opens the page at. */
  | { kind: "page"; id: string; heading?: string }
  | { kind: "memory"; id: string }
  | {
      kind: "citation";
      citation: PageCitation;
      sourceMemory: MemoryItem | null;
      sourcesLoading: boolean;
    };

export interface ReferencePreviewRequest {
  target: ReferenceTarget;
  anchor: HTMLElement;
  keyboard: boolean;
}

export type NavigableReferenceTarget = Extract<ReferenceTarget, { kind: "page" | "memory" }>;

export function referenceTargetFromHref(href: string | undefined): NavigableReferenceTarget | null {
  if (!href) return null;
  const page = href.match(/^#concept:([^\s#~]+)(?:~section:(\S+))?$/);
  if (page) {
    const heading = page[2] ? decodeHeading(page[2]) : "";
    return heading ? { kind: "page", id: page[1], heading } : { kind: "page", id: page[1] };
  }
  const memory = href.match(/^#memory:([^\s#]+)$/);
  if (memory) return { kind: "memory", id: memory[1] };
  return null;
}

function decodeHeading(text: string): string {
  try {
    return decodeURIComponent(text).trim();
  } catch {
    return text.trim();
  }
}
