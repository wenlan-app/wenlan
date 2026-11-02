// SPDX-License-Identifier: AGPL-3.0-only
import { describe, expect, it, vi } from "vitest";
import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { KnowledgeGraph } from "../../../lib/tauri";

const graph: KnowledgeGraph = {
  entities: [], relations: [], memories: [], memory_links: [],
  pages: [
    { id: "pg_gion", title: "Where to stay in Kyoto", space: null, creation_kind: "agent", entity_id: null, last_modified: "2026-11-02T00:00:00Z" },
    { id: "pg_food", title: "Kyoto food", space: null, creation_kind: "agent", entity_id: null, last_modified: "2026-11-02T00:00:00Z" },
  ] as KnowledgeGraph["pages"],
  page_links: [
    { from: { kind: "page", id: "pg_gion" }, to: { kind: "memory", id: "file::/notes/kyoto.md" }, link_type: "cites" },
    { from: { kind: "page", id: "pg_food" }, to: { kind: "memory", id: "file::/notes/other.md" }, link_type: "cites" },
    { from: { kind: "page", id: "pg_food" }, to: { kind: "page", id: "pg_gion" }, link_type: "wikilink" },
  ],
};
vi.mock("../../../lib/tauri", () => ({ getKnowledgeGraph: vi.fn(async () => graph) }));

import { SourceCitedBy } from "./SourceCitedBy";

function renderCitedBy(sourceId: string) {
  const onOpenPage = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><SourceCitedBy sourceId={sourceId} onOpenPage={onOpenPage} /></QueryClientProvider>);
  return onOpenPage;
}

describe("SourceCitedBy", () => {
  it("lists only the pages whose sentences cite this source, and opens one", async () => {
    const onOpenPage = renderCitedBy("file::/notes/kyoto.md");
    const page = await screen.findByRole("button", { name: "Where to stay in Kyoto" });
    expect(screen.queryByRole("button", { name: "Kyoto food" })).not.toBeInTheDocument();
    await userEvent.setup().click(page);
    expect(onOpenPage).toHaveBeenCalledWith("pg_gion");
  });

  it("says so when no page cites the source yet", async () => {
    renderCitedBy("file::/notes/unused.md");
    expect(await screen.findByText("No page cites this yet.")).toBeInTheDocument();
  });
});
