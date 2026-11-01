// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getThing, listThings, listWorkingOn, type ThingSummary } from "../../../lib/tauri";
import { WorkingOn } from "./WorkingOn";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listThings: vi.fn(),
  listWorkingOn: vi.fn(),
  getThing: vi.fn(),
}));

const now = Math.floor(Date.now() / 1000);

function thing(overrides: Partial<ThingSummary> = {}): ThingSummary {
  return {
    id: "thg_1",
    title: "New tagline",
    title_by: "ai",
    status_line: "Picked the X wording; the site still shows the old line.",
    spaces: ["wenlan"],
    last_activity_at: now - 3600,
    ...overrides,
  };
}

function renderWorkingOn(onOpenThing = vi.fn(), onSelectSpace = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <WorkingOn onSelectSpace={onSelectSpace} onOpenThing={onOpenThing} />
    </QueryClientProvider>,
  );
  return { onOpenThing, onSelectSpace };
}

describe("WorkingOn", () => {
  beforeEach(() => {
    vi.mocked(listThings).mockReset();
    vi.mocked(listWorkingOn).mockReset().mockResolvedValue([]);
    vi.mocked(getThing).mockReset().mockImplementation(async (id) => ({ ...thing({ id }), sessions: [], pages: [] }));
  });

  it("shows a tool by its mark, and a tool without one by its name", async () => {
    vi.mocked(listThings).mockResolvedValue([thing()]);
    vi.mocked(getThing).mockResolvedValue({
      ...thing(),
      sessions: [
        { id: "s1", tool: "claude-code", title: null, last_write_at: now },
        { id: "s2", tool: "codex", title: null, last_write_at: now - 60 },
      ],
      pages: [],
    });
    renderWorkingOn();
    const mark = await screen.findByRole("img", { name: "Claude Code" });
    const meta = mark.closest(".wiki-working-on-meta")!;
    expect(meta.querySelector(".wiki-working-on-where")?.textContent).toBe("ChatGPT / Codex · wenlan");
  });

  it("UX-HOME-1: each thing shows where it stands and opens that thing", async () => {
    vi.mocked(listThings).mockResolvedValue([
      thing(),
      thing({ id: "thg_2", title: null, status_line: null, spaces: [] }),
    ]);
    const { onOpenThing } = renderWorkingOn();
    const section = await screen.findByTestId("wiki-working-on");
    const titles = [...section.querySelectorAll(".wiki-working-on-title")].map((el) => el.textContent);
    expect(titles).toEqual(["New tagline", "Not named yet"]);
    expect(section.textContent).toContain("Picked the X wording");
    expect(section.textContent).not.toMatch(/\b\d+ (pages|conversations|sources)\b/);
    fireEvent.click(screen.getByRole("button", { name: /New tagline/ }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_1");
    expect(listWorkingOn).not.toHaveBeenCalled();
  });

  it("UX-HOME-1: falls back to Brief items while there are no things", async () => {
    vi.mocked(listThings).mockResolvedValue([]);
    vi.mocked(listWorkingOn).mockResolvedValue([
      { space: "wenlan", id: "b1", text: "Ship the grey line", added_at: now },
    ]);
    const { onSelectSpace } = renderWorkingOn();
    fireEvent.click(await screen.findByRole("button", { name: /Ship the grey line/ }));
    expect(onSelectSpace).toHaveBeenCalledWith("wenlan");
  });

  it("UX-HOME-3: shows nothing when there is no work", async () => {
    vi.mocked(listThings).mockResolvedValue([]);
    renderWorkingOn();
    await waitFor(() => expect(listWorkingOn).toHaveBeenCalled());
    expect(screen.queryByTestId("wiki-working-on")).toBeNull();
  });
});
