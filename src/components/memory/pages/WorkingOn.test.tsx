// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getThing, hideThing, listThings, listWorkingOn, type ThingSummary } from "../../../lib/tauri";
import { WorkingOn } from "./WorkingOn";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listThings: vi.fn(),
  listWorkingOn: vi.fn(),
  getThing: vi.fn(),
  hideThing: vi.fn(),
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
    vi.mocked(hideThing).mockReset();
  });

  it("shows a tool by its mark, and a tool without one by its name", async () => {
    vi.mocked(listThings).mockResolvedValue([thing()]);
    vi.mocked(getThing).mockResolvedValue({
      ...thing(),
      sessions: [
        { id: "s1", tool: "claude-code", title: null, last_write_at: now },
        { id: "s2", tool: "codex", title: null, last_write_at: now - 60 },
        { id: "s3", tool: "kiro", title: null, last_write_at: now - 120 },
      ],
      pages: [],
    });
    renderWorkingOn();
    const mark = await screen.findByRole("img", { name: "Claude Code" });
    expect(screen.getByRole("img", { name: "ChatGPT / Codex" })).toBeInTheDocument();
    const meta = mark.closest(".wiki-working-on-meta")!;
    expect(meta.querySelector(".wiki-working-on-where")?.textContent).toBe("Kiro · wenlan");
  });

  it("does not name the tool again when its mark is on the card", async () => {
    vi.mocked(listThings).mockResolvedValue([
      thing({ status_line: null }),
      thing({ id: "thg_2", title: "Old notes", status_line: null }),
    ]);
    vi.mocked(getThing).mockImplementation(async (id) => ({
      ...thing({ id, status_line: null }),
      sessions: [{ id: `s-${id}`, tool: id === "thg_1" ? "claude-code" : "kiro", title: null, last_write_at: now }],
      pages: [],
    }));
    renderWorkingOn();
    await screen.findByRole("img", { name: "Claude Code" });
    const cards = [...(await screen.findByTestId("wiki-working-on")).querySelectorAll(".wiki-working-on-card")];
    await waitFor(() => expect(cards[1].querySelector(".wiki-working-on-text")?.textContent).toMatch(/Kiro/));
    expect(cards[0].querySelector(".wiki-working-on-text")?.textContent).toBe("");
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
    fireEvent.click(screen.getByRole("button", { name: /^New tagline/ }));
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

  it("UX-HOME-5: a thing quiet for a week leaves the list, and the Briefs stay out too", async () => {
    vi.mocked(listThings).mockResolvedValue([
      thing(),
      thing({ id: "thg_old", title: "Old import", quiet: true, last_activity_at: now - 40 * 86400 }),
    ]);
    renderWorkingOn();
    const section = await screen.findByTestId("wiki-working-on");
    expect(section.textContent).toContain("New tagline");
    expect(section.textContent).not.toContain("Old import");
    expect(screen.queryByRole("button", { name: "See all" })).toBeNull();
  });

  it("UX-HOME-5: when every thing is quiet, home shows nothing rather than old Brief items", async () => {
    vi.mocked(listThings).mockResolvedValue([thing({ quiet: true })]);
    vi.mocked(listWorkingOn).mockResolvedValue([
      { space: "wenlan", id: "b1", text: "Ship the grey line", added_at: now - 60 * 86400 },
    ]);
    renderWorkingOn();
    await waitFor(() => expect(listThings).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(screen.queryByTestId("wiki-working-on")).toBeNull();
    expect(listWorkingOn).not.toHaveBeenCalled();
  });

  it("UX-HOME-5: no cap: past six, See all opens the rest in place, with no count", async () => {
    vi.mocked(listThings).mockResolvedValue(
      Array.from({ length: 9 }, (_, i) => thing({ id: `thg_${i}`, title: `Work ${i}` })),
    );
    renderWorkingOn();
    const section = await screen.findByTestId("wiki-working-on");
    expect(section.querySelectorAll(".wiki-working-on-card")).toHaveLength(6);
    const all = screen.getByRole("button", { name: "See all" });
    expect(all).toHaveAttribute("aria-expanded", "false");
    expect(section.textContent).not.toMatch(/\d+ more/);
    fireEvent.click(all);
    expect(section.querySelectorAll(".wiki-working-on-card")).toHaveLength(9);
    expect(section).toHaveAttribute("data-expanded", "true");
    fireEvent.click(screen.getByRole("button", { name: "Show fewer" }));
    expect(section.querySelectorAll(".wiki-working-on-card")).toHaveLength(6);
  });

  it("UX-HOME-5: Don't show takes a thing off home at once, without opening it", async () => {
    let list = [thing(), thing({ id: "thg_mac", title: "New Mac" })];
    vi.mocked(listThings).mockImplementation(async () => list);
    vi.mocked(hideThing).mockImplementation(async (id) => {
      list = list.map((t) => (t.id === id ? { ...t, quiet: true } : t));
      return list.find((t) => t.id === id)!;
    });
    const { onOpenThing } = renderWorkingOn();
    const section = await screen.findByTestId("wiki-working-on");
    await waitFor(() => expect(section.textContent).toContain("New Mac"));
    fireEvent.click(screen.getByRole("button", { name: "Don't show New Mac" }));
    await waitFor(() => expect(hideThing).toHaveBeenCalledWith("thg_mac"));
    await waitFor(() => expect(section.textContent).not.toContain("New Mac"));
    expect(section.textContent).toContain("New tagline");
    expect(onOpenThing).not.toHaveBeenCalled();
  });
});
