// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import type { SkillLine, Space } from "../../../lib/tauri";

const api = vi.hoisted(() => ({
  enabled: vi.fn(),
  list: vi.fn(),
  accept: vi.fn(),
  decline: vi.fn(),
}));

vi.mock("../../../lib/tauri", () => ({
  skillsFromLibraryEnabled: api.enabled,
  listSpaceRulesSuggestions: api.list,
  acceptSpaceRules: api.accept,
  declineSpaceRules: api.decline,
}));

import { SpaceRulesAsk } from "./SpaceRulesAsk";

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
const line: SkillLine = {
  id: "rule-1",
  kind: "rule",
  text: "Discuss the approach before writing code",
  source_page_id: "page-source",
  source_page_title: "Working style",
  source_revision: 2,
  source_revision_at: 1_799_000_000,
  source_quote: "Discuss the approach before writing code",
  person_edited: false,
};

function renderAsk(onOpenPage = vi.fn()) {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const result = render(
    <QueryClientProvider client={queryClient}>
      <SpaceRulesAsk space={space} onOpenPage={onOpenPage} />
    </QueryClientProvider>,
  );
  return { ...result, onOpenPage };
}

describe("Space how-I-work ask", () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.enabled.mockResolvedValue(true);
    api.list.mockResolvedValue([{ space_id: space.id, space_name: space.name, lines: [line] }]);
    api.accept.mockResolvedValue({ space_id: space.id, status: "accepted", rules_page_id: "rules-page", undo_available: false });
    api.decline.mockResolvedValue({ space_id: space.id, status: "declined", rules_page_id: null, undo_available: false });
  });

  it("UX-SKILL-1 shows the one-time ask above a grounded line and opens its source", async () => {
    const { onOpenPage } = renderAsk();
    expect(await screen.findByRole("heading", { name: /Let AI tools follow how you work/ })).toBeInTheDocument();
    expect(screen.getByText(line.text)).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Working style" }));
    expect(onOpenPage).toHaveBeenCalledExactlyOnceWith("page-source");
  });

  it("UX-SKILL-1 always names the source in the ask, never an edit mark", async () => {
    api.list.mockResolvedValue([{ space_id: space.id, space_name: space.name, lines: [{ ...line, person_edited: true }] }]);
    renderAsk();
    expect(await screen.findByRole("button", { name: "Working style" })).toBeInTheDocument();
    expect(screen.queryByText("You changed this")).not.toBeInTheDocument();
  });

  it("UX-SKILL-1 remembers decline so the ask never returns", async () => {
    let pending = true;
    api.list.mockImplementation(async () => pending ? [{ space_id: space.id, space_name: space.name, lines: [line] }] : []);
    api.decline.mockImplementation(async () => {
      pending = false;
      return { space_id: space.id, status: "declined", rules_page_id: null, undo_available: false };
    });
    const first = renderAsk();
    fireEvent.click(await screen.findByRole("button", { name: "No thanks" }));
    await waitFor(() => expect(screen.queryByTestId("space-rules-ask")).not.toBeInTheDocument());
    first.unmount();
    renderAsk();
    await waitFor(() => expect(api.list).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId("space-rules-ask")).not.toBeInTheDocument();
  });

  it("UX-SKILL-1 shows nothing when the setting is off", async () => {
    api.enabled.mockResolvedValue(false);
    renderAsk();
    await waitFor(() => expect(api.enabled).toHaveBeenCalled());
    expect(api.list).not.toHaveBeenCalled();
    expect(screen.queryByTestId("space-rules-ask")).not.toBeInTheDocument();
  });

  it("UX-SKILL-1 stays hidden when this Space has no suggestion", async () => {
    api.list.mockResolvedValue([]);
    renderAsk();
    await waitFor(() => expect(api.list).toHaveBeenCalled());
    expect(screen.queryByTestId("space-rules-ask")).not.toBeInTheDocument();
  });
});
