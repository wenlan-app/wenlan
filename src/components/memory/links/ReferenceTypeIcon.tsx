// SPDX-License-Identifier: AGPL-3.0-only
import { Brain } from "@phosphor-icons/react";
import type { PageCitation } from "../../../lib/tauri";
import { PageIcon } from "../assets/PageIcon";
import { SourceTypeIcon } from "../sources/SourceTypeIcon";
import { getSourceAppFromUrl, getSourceKind, type SourceApp } from "../sources/sourcePresentation";

export type ReferenceKind = "page" | "memory" | "file" | "url" | "authored";

export function citationReferenceKind(citation: PageCitation): ReferenceKind {
  switch (citation.source_kind) {
    case "memory": return "memory";
    case "external_file": return "file";
    case "external_url": return "url";
    case "authored": return "authored";
  }
}

/** The app a web citation came from (Gmail, Notion, ...), or null for any other link. */
export function citationSourceApp(citation: PageCitation): SourceApp | null {
  return citation.source_kind === "external_url" ? getSourceAppFromUrl(citation.locator) : null;
}

export function ReferenceTypeIcon({ kind, size = 13, locator }: { kind: ReferenceKind; size?: number; locator?: string }) {
  if (kind === "memory") return <Brain size={size} weight="regular" aria-hidden="true" focusable="false" />;
  if (kind === "page" || kind === "authored") return <PageIcon size={size} />;
  const app = kind === "url" ? getSourceAppFromUrl(locator) : null;
  if (app) return <SourceTypeIcon kind={app} size={size} />;
  const sourceKind = getSourceKind({ source: kind === "url" ? "webpage" : "file", source_id: locator ?? "", title: "" });
  return <SourceTypeIcon kind={sourceKind} size={size} />;
}
