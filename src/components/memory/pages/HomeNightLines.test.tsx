// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { i18n } from "../../../i18n";
import {
  clipboardWrite,
  listAgents,
  listSetAsideMaterial,
  listThings,
  openToolWithPrompt,
  releaseSetAsideMaterial,
  type AgentConnection,
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
}));

const now = Math.floor(Date.now() / 1000);

function agent(name: string): AgentConnection {
  return {
    id: `agent-${name}`, name, display_name: null, agent_type: name, description: null, enabled: true,
    trust_level: "full", last_seen_at: now, memory_count: 0, created_at: 1, updated_at: 1,
  };
}

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
    vi.mocked(releaseSetAsideMaterial).mockReset().mockResolvedValue({ released: 3 });
    vi.mocked(openToolWithPrompt).mockReset().mockResolvedValue(undefined);
    vi.mocked(clipboardWrite).mockReset().mockResolvedValue(undefined);
    await i18n.changeLanguage("en");
  });
  afterEach(async () => {
    await i18n.changeLanguage("en");
  });

  it("UX-HOME-7 shows the block when only set-aside material exists", async () => {
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [group("Driftwood", ["itm_1"])] });
    renderLines();
    const line = await screen.findByTestId("home-unwritten");
    expect(line).toHaveTextContent("Not written yet:");
    expect(line).toHaveTextContent("Driftwood");
    expect(screen.getByTestId("home-night")).toBeInTheDocument();
    expect(screen.queryByTestId("home-steeped")).toBeNull();
  });

  it("UX-HOME-7 is absent when nothing was set aside and nothing steeped", async () => {
    renderLines();
    await waitFor(() => expect(listSetAsideMaterial).toHaveBeenCalled());
    await waitFor(() => expect(listThings).toHaveBeenCalled());
    expect(screen.queryByTestId("home-night")).toBeNull();
    expect(screen.queryByTestId("home-unwritten")).toBeNull();
  });

  it("UX-HOME-7 sits under the steeped line in the same block", async () => {
    vi.mocked(listThings).mockResolvedValue([
      { id: "t1", title: "Home redesign", title_by: "ai", status_line: null, spaces: [], last_activity_at: now, quiet: false, dreamed_at: now - 60 } as ThingSummary,
    ]);
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [group("Driftwood", ["itm_1"])] });
    renderLines();
    const unwritten = await screen.findByTestId("home-unwritten");
    const steeped = screen.getByTestId("home-steeped");
    expect(steeped.nextElementSibling).toBe(unwritten);
    expect(unwritten).toHaveClass("home-night-line");
  });

  it("UX-HOME-2 UX-HOME-7 names at most two groups, then 'and more', never a count", async () => {
    vi.mocked(listSetAsideMaterial).mockResolvedValue({
      groups: [group("Driftwood", ["a"]), group("The Design of Everyday Things", ["b", "c"]), group("Kyoto", ["d"])],
    });
    renderLines();
    const line = await screen.findByTestId("home-unwritten");
    expect(line.querySelector(".home-night-names")?.textContent).toBe("Driftwood, The Design of Everyday Things and more");
    expect(line).not.toHaveTextContent("Kyoto");
    expect(line.textContent).not.toMatch(/\d/);
  });

  it("UX-HOME-7 hands the connected tool its steep command for exactly these groups, named for that tool", async () => {
    vi.mocked(listAgents).mockResolvedValue([agent("codex")]);
    vi.mocked(listSetAsideMaterial).mockResolvedValue({
      groups: [group("Driftwood", ["itm_1", "itm_2"]), group("The Design of Everyday Things", ["itm_3"], "books")],
    });
    renderLines();
    const action = await screen.findByTestId("home-unwritten-action");
    await waitFor(() => expect(action).toHaveTextContent("Ask Codex"));
    expect(action).not.toHaveTextContent("Claude");
    fireEvent.click(action);
    await waitFor(() => expect(openToolWithPrompt).toHaveBeenCalled());
    const [family, text] = vi.mocked(openToolWithPrompt).mock.calls[0];
    expect(family).toBe("codex");
    expect(text).toBe(
      "$wenlan write my pages — only the groups Wenlan set aside: "
      + "claim_new item_ids [\"itm_1\",\"itm_2\"] titled «Driftwood» in Space «reading»; "
      + "claim_new item_ids [\"itm_3\"] titled «The Design of Everyday Things» in Space «books». "
      + "Claim exactly these items and leave other waiting material alone.",
    );
    expect(releaseSetAsideMaterial).not.toHaveBeenCalled();
  });

  it("UX-HOME-7 words the button in Chinese with the tool's own name", async () => {
    await i18n.changeLanguage("zh-Hant");
    vi.mocked(listAgents).mockResolvedValue([agent("claude-code")]);
    vi.mocked(listSetAsideMaterial).mockResolvedValue({ groups: [group("Driftwood", ["a"]), group("設計的心理學", ["b"])] });
    renderLines();
    const line = await screen.findByTestId("home-unwritten");
    await waitFor(() => expect(screen.getByTestId("home-unwritten-action")).toHaveTextContent("交給 Claude Code"));
    expect(line).toHaveTextContent("還沒寫成頁");
    expect(line.querySelector(".home-night-names")?.textContent).toBe("Driftwood、設計的心理學");
  });

  it("UX-HOME-7 with no AI tool connected, Try again gives every listed item back and reads the list again", async () => {
    vi.mocked(listSetAsideMaterial)
      .mockResolvedValueOnce({ groups: [group("Driftwood", ["itm_1", "itm_2"]), group("Kyoto", ["itm_3"])] })
      .mockResolvedValue({ groups: [] });
    renderLines();
    const action = await screen.findByTestId("home-unwritten-action");
    await waitFor(() => expect(action).toHaveTextContent("Try again"));
    fireEvent.click(action);
    await waitFor(() => expect(releaseSetAsideMaterial).toHaveBeenCalledWith(["itm_1", "itm_2", "itm_3"]));
    await waitFor(() => expect(screen.queryByTestId("home-unwritten")).toBeNull());
    expect(openToolWithPrompt).not.toHaveBeenCalled();
  });
});
