// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Page, SkillSuggestion, Space } from "../../../lib/tauri";

const api = vi.hoisted(() => ({
  enabled: vi.fn(),
  getSpace: vi.fn(),
  records: vi.fn(),
  suggestions: vi.fn(),
  refresh: vi.fn(),
  undoNight: vi.fn(),
  apply: vi.fn(),
  dismiss: vi.fn(),
  undoSuggestion: vi.fn(),
  updatePage: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => ({
  skillsFromLibraryEnabled: api.enabled,
  getSpace: api.getSpace,
  listSkillBorrowRecords: api.records,
  listSkillSuggestions: api.suggestions,
  refreshSkills: api.refresh,
  undoSpaceRules: api.undoNight,
  applySkillSuggestion: api.apply,
  dismissSkillSuggestion: api.dismiss,
  undoSkillSuggestion: api.undoSuggestion,
  updatePage: api.updatePage,
}));

import { HowIWorkPage } from "./HowIWorkPage";

const now = Math.floor(Date.now() / 1000);
const content = `# How I work — Wenlan

These are this Space's approved working rules. Wenlan suggests the first version; only the person accepts it.

- [rule] Discuss the approach before writing code — Source: Working style (page-source-1) r2 at ${now - 86400} — Quote: "Discuss the approach before writing code" <!-- wenlan-rule:rule-1 -->
- [decision] Put completed work into the merge queue — Source: Release flow (page-source-2) r4 at ${now - 43200} — Quote: "Push completed work directly to main" <!-- wenlan-rule:decision-1 -->
`;
const page: Page = {
  id: "rules-page",
  title: "How I work — Wenlan",
  summary: null,
  content,
  entity_id: null,
  domain: "Wenlan",
  space: "Wenlan",
  source_memory_ids: [],
  version: 3,
  status: "active",
  creation_kind: "authored",
  created_at: new Date((now - 86400) * 1000).toISOString(),
  last_compiled: new Date((now - 3600) * 1000).toISOString(),
  last_modified: new Date((now - 3600) * 1000).toISOString(),
};
const space: Space = {
  id: "space-wenlan",
  name: "Wenlan",
  description: null,
  suggested: false,
  starred: false,
  sort_order: 0,
  memory_count: 0,
  entity_count: 0,
  created_at: 1,
  updated_at: 1,
};
const suggestion: SkillSuggestion = {
  id: "suggestion-1",
  skill: "release",
  old_line: "Push completed work directly to main",
  new_line: "Put completed work into the merge queue",
  reason: "A newer decision changed the release flow",
  source_rule_id: "decision-1",
  source_page: "Release flow",
};

function renderPage(onOpenPage = vi.fn()) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return {
    onOpenPage,
    ...render(
      <QueryClientProvider client={queryClient}>
        <HowIWorkPage page={page} onOpenPage={onOpenPage} />
      </QueryClientProvider>,
    ),
  };
}

describe("How-I-work page", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.enabled.mockResolvedValue(true);
    api.getSpace.mockResolvedValue(space);
    api.records.mockResolvedValue([
      { at: now - 1800, tool: "codex", space_id: space.id, page_title: "Release flow", rule_id: "decision-1", self_report: "changed_plan" },
      { at: now - 3600, tool: "some-tool", space_id: space.id, page_title: "Working style", rule_id: "rule-1" },
    ]);
    api.suggestions.mockResolvedValue([suggestion]);
    api.refresh.mockResolvedValue({ spaces_considered: 1, spaces_unchanged: 1, spaces_updated: 0, suggestions: 0, failures: 0, model_calls: 0 });
    api.undoNight.mockResolvedValue({ space_id: space.id, status: "undone", rules_page_id: page.id, undo_available: false });
    api.apply.mockResolvedValue({ suggestion, applied: true, undo_available: true });
    api.dismiss.mockResolvedValue({ suggestion, applied: false, undo_available: false });
    api.undoSuggestion.mockResolvedValue({ suggestion, applied: false, undo_available: false });
    api.updatePage.mockResolvedValue({ outcome: "saved" });
  });

  it("UX-SKILL-2 shows actions, real borrowing labels and person-edited lines without internal ids", async () => {
    const { onOpenPage } = renderPage();
    expect(await screen.findByRole("button", { name: "Update now" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Undo last night" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    const borrowed = await screen.findByRole("heading", { name: "AI tools borrowed this week" });
    const borrowedSection = borrowed.closest("section")!;
    expect(within(borrowedSection).getByText(/Reported by the AI tool/)).toBeInTheDocument();
    expect(within(borrowedSection).getByText(/Recorded by Wenlan/)).toBeInTheDocument();
    expect(screen.getByText("You changed this")).toBeInTheDocument();
    expect(document.body).not.toHaveTextContent("page-source-1");
    fireEvent.click(screen.getByRole("button", { name: "Working style" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-source-1");
  });

  it("UX-SKILL-2 shows no borrowing section when the record is empty and Stop only adds the opt-out marker", async () => {
    api.records.mockResolvedValue([]);
    renderPage();
    await screen.findByRole("button", { name: "Stop" });
    expect(screen.queryByRole("heading", { name: "AI tools borrowed this week" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Stop" }));
    await waitFor(() => expect(api.updatePage).toHaveBeenCalled());
    const request = api.updatePage.mock.calls[0][0];
    expect(request.content).toContain("<!-- wenlan:not-for-ai-tools -->");
    expect(request.content).toContain("<!-- wenlan-rule:rule-1 -->");
  });

  it("UX-SKILL-3 keeps an own-skill conflict suggestion-only, then offers Undo after Apply", async () => {
    renderPage();
    const title = await screen.findByText("Your skill “release” has a line that differs from a newer decision");
    const card = title.closest("article")!;
    expect(within(card).getByText(suggestion.old_line)).toHaveClass("how-i-work-suggestion-old");
    expect(within(card).getByText(suggestion.new_line)).toBeInTheDocument();
    expect(api.apply).not.toHaveBeenCalled();
    fireEvent.click(within(card).getByRole("button", { name: "Change it" }));
    await waitFor(() => expect(api.apply).toHaveBeenCalledExactlyOnceWith(suggestion.id));
    const undo = within(card).getByRole("button", { name: "Undo" });
    fireEvent.click(undo);
    await waitFor(() => expect(api.undoSuggestion).toHaveBeenCalledExactlyOnceWith(suggestion.id));
    await waitFor(() => expect(screen.queryByText(title.textContent ?? "")).not.toBeInTheDocument());
  });
});
