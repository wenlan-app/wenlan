// SPDX-License-Identifier: AGPL-3.0-only
import { useMemo } from "react";
import { useQuery } from "@tanstack/react-query";
import { useTranslation } from "react-i18next";
import { getKnowledgeGraph } from "../../../lib/tauri";
import { PageIcon } from "../assets/PageIcon";

/**
 * The pages whose sentences cite this source. A source is what a sentence
 * cites, so the person reads it from here rather than from a list of sources.
 */
export function SourceCitedBy({ sourceId, onOpenPage }: {
  readonly sourceId: string;
  readonly onOpenPage: (pageId: string) => void;
}) {
  const { t } = useTranslation();
  // Shares the graph the page sidebars already read.
  const graph = useQuery({ queryKey: ["knowledge-graph"], queryFn: getKnowledgeGraph, staleTime: 0, retry: false });
  const pages = useMemo(() => {
    if (!graph.data) return [];
    const ids = new Set(graph.data.page_links
      .filter((link) => link.link_type === "cites" && link.to.kind === "memory" && link.to.id === sourceId && link.from.kind === "page")
      .map((link) => link.from.id));
    return graph.data.pages.filter((page) => ids.has(page.id));
  }, [graph.data, sourceId]);
  if (!graph.data) return null;
  return (
    <section className="source-cited-by" aria-label={t("sourceAccess.citedBy")}>
      <h3 className="note-info-section-heading">{t("sourceAccess.citedBy")}</h3>
      {pages.length === 0
        ? <p className="source-cited-by-none">{t("sourceAccess.citedByNone")}</p>
        : <ul className="note-info-source-list">
          {pages.map((page) => (
            <li key={page.id}>
              <button type="button" className="page-info-source-button source-cited-by-page" onClick={() => onOpenPage(page.id)}>
                <PageIcon size={14} />
                <span>{page.title || t("knowledgeContext.untitled")}</span>
              </button>
            </li>
          ))}
        </ul>}
    </section>
  );
}
