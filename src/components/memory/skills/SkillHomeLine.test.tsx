// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page } from "../../../lib/tauri";

const api = vi.hoisted(() => ({ enabled: vi.fn(), records: vi.fn() }));
vi.mock("../../../lib/tauri", () => ({
  skillsFromLibraryEnabled: api.enabled,
  listSkillBorrowRecords: api.records,
}));

import { SkillHomeLine } from "./SkillHomeLine";

const now = new Date();
const page = (id: string, daysAgo: number): Page => ({
  id,
  title: "How I work — Wenlan",
  summary: null,
  content: "# How I work — Wenlan\n\nThese are this Space's approved working rules.\n\n- [rule] Use the merge queue — Source: Release flow (source) r1 at 1 — Quote: \"Use the merge queue\" <!-- wenlan-rule:rule-1 -->\n",
  entity_id: null,
  domain: "Wenlan",
  space: "Wenlan",
  source_memory_ids: [],
  version: 1,
  status: "active",
  creation_kind: "authored",
  created_at: new Date(now.getTime() - daysAgo * 86_400_000).toISOString(),
  last_compiled: now.toISOString(),
  last_modified: now.toISOString(),
});

function renderLine(pages: Page[], onOpenPage = vi.fn()) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return {
    onOpenPage,
    ...render(
      <QueryClientProvider client={queryClient}>
        <SkillHomeLine pages={pages} onOpenPage={onOpenPage} />
      </QueryClientProvider>,
    ),
  };
}

describe("Home how-I-work line", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.enabled.mockResolvedValue(true);
    api.records.mockResolvedValue([]);
  });

  it("UX-SKILL-4 shows one quiet, count-free line for a changed-plan record and opens the rules page", async () => {
    api.records.mockResolvedValue([{ at: Math.floor(Date.now() / 1000), tool: "codex", space_id: "space-1", page_title: "Release flow", rule_id: "rule-1", self_report: "changed_plan" }]);
    const old = page("rules-old", 30);
    const { onOpenPage } = renderLine([old, { ...old, id: "rules-older", space: "Other", title: "How I work — Other" }]);
    const line = await screen.findByTestId("skill-home-line");
    expect(line).toHaveTextContent("This week an AI tool changed its approach to follow an earlier decision · Take a look");
    expect(line.textContent).not.toMatch(/\d/);
    expect(screen.getAllByTestId("skill-home-line")).toHaveLength(1);
    fireEvent.click(screen.getByRole("button", { name: "Take a look" }));
    expect(onOpenPage).toHaveBeenCalledTimes(1);
  });

  it("UX-SKILL-4 shows the same single line when the first rules page was made this week", async () => {
    renderLine([page("rules-new", 0)]);
    expect(await screen.findByTestId("skill-home-line")).toBeInTheDocument();
  });

  it("UX-SKILL-4 stays absent without a changed-plan record or a newly made rules page", async () => {
    renderLine([page("rules-old", 30)]);
    await waitFor(() => expect(api.records).toHaveBeenCalled());
    expect(screen.queryByTestId("skill-home-line")).not.toBeInTheDocument();
  });
});
