// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  getAiMemoryImport,
  stopAiMemoryImport,
  listAgents,
  listSetAsideMaterial,
  listThings,
  type SetAsideGroup,
  type ThingSummary,
} from "../../../lib/tauri";
import { HomeNightLines } from "./HomeNightLines";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listThings: vi.fn(),
  listAgents: vi.fn(),
  listSetAsideMaterial: vi.fn(),
  releaseSetAsideMaterial: vi.fn(),
  openToolWithPrompt: vi.fn(),
  clipboardWrite: vi.fn(),
  setUiLanguage: vi.fn(() => Promise.resolve()),
  getAiMemoryImport: vi.fn(() => Promise.resolve({ found: [], ready: false, import: null })),
  stopAiMemoryImport: vi.fn(() => Promise.resolve({ undone: null })),
}));

const now = Math.floor(Date.now() / 1000);

function group(name: string, ids: string[], space = "reading"): SetAsideGroup {
  return {
    name,
    reason: "too_short",
    items: ids.map((id) => ({ item_id: id, title: `${name} ${id}`, kind: "note", space })),
    set_aside_at: now - 60,
  };
}

function renderLines() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <HomeNightLines onReplayNight={vi.fn()} />
    </QueryClientProvider>,
  );
}

describe("HomeNightLines: not written yet", () => {
  beforeEach(async () => {
    vi.mocked(listThings).mockReset().mockResolvedValue([]);
    vi.mocked(listAgents).mockReset().mockResolvedValue([]);
    vi.mocked(listSetAsideMaterial).mockReset().mockResolvedValue({ groups: [] });
    await i18n.changeLanguage("en");
  });

  it("UX-HOME-7 home says nothing about set-aside material; it lives on Activity", async () => {
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [group("Driftwood", ["itm_1"])] });
    renderLines();
    await waitFor(() => expect(listThings).toHaveBeenCalled());
    expect(screen.queryByTestId("home-night")).toBeNull();
    expect(screen.queryByText(/Not written yet/)).toBeNull();
    expect(listSetAsideMaterial).not.toHaveBeenCalled();
  });

  it("UX-HOME-7 leaves the steeped line alone on home", async () => {
    vi.mocked(listThings).mockResolvedValue([
      { id: "t1", title: "Home redesign", title_by: "ai", status_line: null, spaces: [], last_activity_at: now, quiet: false, dreamed_at: now - 60 } as ThingSummary,
    ]);
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [group("Driftwood", ["itm_1"])] });
    renderLines();
    const steeped = await screen.findByTestId("home-steeped");
    expect(steeped.nextElementSibling).toBeNull();
    expect(screen.getByTestId("home-night")).not.toHaveTextContent("Driftwood");
  });
});

describe("HomeNightLines: following AI tools' memory", () => {
  beforeEach(() => {
    vi.mocked(listThings).mockResolvedValue([]);
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [] });
    vi.mocked(listAgents).mockResolvedValue([]);
    localStorage.clear();
  });

  it("UX-HOME-2 says once, with no count, that following is on, and Stop turns it off", async () => {
    vi.mocked(getAiMemoryImport).mockResolvedValue({
      found: [{ tool: "codex", memories: 4 }],
      ready: true,
      import: { at: now - 60, last_at: now - 60, counts: {} as never, by_tool: [] },
    });
    renderLines();
    const line = await screen.findByTestId("home-ai-memory");
    expect(line.textContent).not.toMatch(/\d/);
    fireEvent.click(screen.getByRole("button", { name: i18n.t("pages.overview.aiMemoryStop") }));
    await waitFor(() => expect(stopAiMemoryImport).toHaveBeenCalledTimes(1));
    await waitFor(() => expect(screen.queryByTestId("home-ai-memory")).toBeNull());
  });

  it("says nothing a day after following was turned on", async () => {
    vi.mocked(getAiMemoryImport).mockResolvedValue({
      found: [],
      ready: true,
      import: { at: now - 2 * 24 * 60 * 60, last_at: now, counts: {} as never, by_tool: [] },
    });
    renderLines();
    await waitFor(() => expect(getAiMemoryImport).toHaveBeenCalled());
    expect(screen.queryByTestId("home-ai-memory")).toBeNull();
  });
});
