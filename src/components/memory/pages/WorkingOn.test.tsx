// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { getThing, hideThing, listThings, listWorkingOn, setUiLanguage, type ThingSummary } from "../../../lib/tauri";
import { WorkingOn } from "./WorkingOn";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  listThings: vi.fn(),
  listWorkingOn: vi.fn(),
  getThing: vi.fn(),
  hideThing: vi.fn(),
  setUiLanguage: vi.fn(() => Promise.resolve()),
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

function renderWorkingOn(onOpenThing = vi.fn(), onSelectSpace = vi.fn(), onReplayNight?: () => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <WorkingOn onSelectSpace={onSelectSpace} onOpenThing={onOpenThing} onReplayNight={onReplayNight} />
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

  it("UX-HOME-6: shows a tool by its mark, and a tool without one by its name, and no Space", async () => {
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
    expect(meta.querySelector(".wiki-working-on-where")?.textContent).toBe("Kiro");
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
    expect(titles).toEqual(["New tagline"]);
    expect(section).not.toHaveTextContent("Not named yet");
    expect(section.textContent).toContain("Picked the X wording");
    expect(section.textContent).not.toMatch(/\b\d+ (pages|conversations|sources)\b/);
    fireEvent.click(screen.getByRole("button", { name: /^New tagline/ }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_1");
    expect(listWorkingOn).not.toHaveBeenCalled();
  });

  it("does not present unnamed work as a home card or fall back to an old Brief", async () => {
    vi.mocked(listThings).mockResolvedValue([
      thing({ id: "thg_unnamed", title: null, status_line: null, spaces: [] }),
    ]);
    vi.mocked(listWorkingOn).mockResolvedValue([
      { space: "wenlan", id: "old-brief", text: "An older Brief", added_at: now - 60 * 86400 },
    ]);
    renderWorkingOn();
    await waitFor(() => expect(listThings).toHaveBeenCalled());
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(screen.queryByTestId("wiki-working-on")).toBeNull();
    expect(screen.queryByText("Not named yet")).toBeNull();
    expect(screen.queryByText("An older Brief")).toBeNull();
    expect(listWorkingOn).not.toHaveBeenCalled();
  });

  it("UX-THING-1: tells Wenlan the app's language, which the things it reads are named in", async () => {
    vi.mocked(listThings).mockResolvedValue([thing()]);
    renderWorkingOn();
    await screen.findByTestId("wiki-working-on");
    await waitFor(() => expect(setUiLanguage).toHaveBeenCalledWith(expect.stringMatching(/^(en|zh)/)));
  });

  it("UX-HOME-6: Brief items never fill home, even while there are no things", async () => {
    vi.mocked(listThings).mockResolvedValue([]);
    vi.mocked(listWorkingOn).mockResolvedValue([
      { space: "wenlan", id: "b1", text: "Ship the grey line", added_at: now },
    ]);
    renderWorkingOn();
    await waitFor(() => expect(listThings).toHaveBeenCalled());
    expect(listWorkingOn).not.toHaveBeenCalled();
    expect(screen.queryByTestId("wiki-working-on")).toBeNull();
  });

  it("UX-HOME-3: shows nothing when there is no work", async () => {
    vi.mocked(listThings).mockResolvedValue([]);
    renderWorkingOn();
    await waitFor(() => expect(listThings).toHaveBeenCalled());
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

  it("UX-CHAT-5: names what was steeped overnight beside the heading, and pressing a name lights its card", async () => {
    const tonight = new Date();
    tonight.setHours(3, 0, 0, 0);
    const at = Math.floor(tonight.getTime() / 1000);
    vi.mocked(listThings).mockResolvedValue([
      thing({ id: "thg_1", title: "Site redesign", dreamed_at: at }),
      thing({ id: "thg_2", title: "Release notes", dreamed_at: at - 86_400 * 2 }),
      thing({ id: "thg_3", title: "Old work", quiet: true, dreamed_at: at }),
    ]);
    const { onOpenThing } = renderWorkingOn();
    const line = await screen.findByTestId("wiki-working-on-steeped");
    expect(line).toHaveTextContent("Steeped overnight:");
    expect(line).toHaveTextContent("Site redesign, Old work");
    expect(line).not.toHaveTextContent("Release notes");
    // Names, never a count.
    expect(line.textContent).not.toMatch(/\d/);
    fireEvent.click(screen.getByRole("button", { name: "Site redesign" }));
    expect(onOpenThing).not.toHaveBeenCalled();
    const card = document.querySelector('[data-thing-id="thg_1"] .wiki-working-on-card');
    expect(card).toHaveAttribute("data-glow", "true");
    // A quiet thing has no card: its name opens it.
    fireEvent.click(screen.getByRole("button", { name: "Old work" }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_3");
  });

  it("UX-GRAPH-5: home's names line adds only Replay: no count and no Undo", async () => {
    const tonight = new Date();
    tonight.setHours(3, 0, 0, 0);
    vi.mocked(listThings).mockResolvedValue([thing({ id: "thg_1", title: "Site redesign", dreamed_at: Math.floor(tonight.getTime() / 1000) })]);
    const onReplayNight = vi.fn();
    renderWorkingOn(vi.fn(), vi.fn(), onReplayNight);
    const line = await screen.findByTestId("wiki-working-on-steeped");
    expect(line.textContent).toBe("Steeped overnight:Site redesign·Replay");
    expect(line.textContent).not.toMatch(/\d/);
    expect(screen.queryByRole("button", { name: "Undo" })).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Replay" }));
    expect(onReplayNight).toHaveBeenCalledTimes(1);
  });

  it("UX-CHAT-5: names two steeped things at most, and \"and more\" lights every steeped card", async () => {
    const tonight = new Date();
    tonight.setHours(3, 0, 0, 0);
    const at = Math.floor(tonight.getTime() / 1000);
    vi.mocked(listThings).mockResolvedValue([
      thing({ id: "thg_1", title: "Site redesign", dreamed_at: at }),
      thing({ id: "thg_2", title: "Release notes", dreamed_at: at }),
      thing({ id: "thg_3", title: "Zotero workflow", dreamed_at: at }),
    ]);
    renderWorkingOn();
    const line = await screen.findByTestId("wiki-working-on-steeped");
    expect(line).toHaveTextContent("Site redesign, Release notesand more");
    expect(line).not.toHaveTextContent("Zotero workflow");
    fireEvent.click(screen.getByRole("button", { name: "and more" }));
    for (const id of ["thg_1", "thg_2", "thg_3"]) {
      expect(document.querySelector(`[data-thing-id="${id}"] .wiki-working-on-card`)).toHaveAttribute("data-glow", "true");
    }
  });

  it("UX-HOME-6: writes a steeped name and a card title out whole, never cut short by the app", async () => {
    const tonight = new Date();
    tonight.setHours(3, 0, 0, 0);
    const at = Math.floor(tonight.getTime() / 1000);
    vi.mocked(listThings).mockResolvedValue([
      thing({ id: "thg_1", title: "Live self-testing of the new map", dreamed_at: at }),
    ]);
    renderWorkingOn();
    const line = await screen.findByTestId("wiki-working-on-steeped");
    expect(line).toHaveTextContent("Live self-testing of the new map");
    expect(line.textContent).not.toContain("…");
    const title = document.querySelector('[data-thing-id="thg_1"] .wiki-working-on-title');
    expect(title).toHaveTextContent("Live self-testing of the new map");
    expect(title).toHaveAttribute("title", "Live self-testing of the new map");
  });

  it("UX-HOME-1, UX-HOME-2: only the newest thing writes out its newest open point, and nothing is counted", async () => {
    vi.mocked(listThings).mockResolvedValue([
      thing({ id: "thg_1", title: "Home layout", last_activity_at: now - 60 }),
      thing({ id: "thg_2", title: "Launch post", last_activity_at: now - 7200 }),
    ]);
    const point = (source_id: string, title: string, ago: number) => ({ source_id, title, created_at: now - ago });
    vi.mocked(getThing).mockImplementation(async (id) => ({
      ...thing({ id }),
      sessions: [],
      pages: [],
      memories: [
        point("mem_a", "Home shows only your own chats", 900),
        point("mem_b", "Should the Brief merge into things?", 800),
        point("mem_c", "Which screenshot goes first?", 700),
      ],
      dream: { at: now - 3600, decided: 1, open: 2, fresh: [] },
      open_points: ["mem_b", "mem_c"],
    }));
    renderWorkingOn();
    const open = await screen.findByTestId("wiki-working-on-open");
    const large = document.querySelector('[data-thing-id="thg_1"]');
    expect(large).toContainElement(open);
    expect(open).toHaveTextContent("Still open:Which screenshot goes first?");
    expect(screen.getAllByTestId("wiki-working-on-open")).toHaveLength(1);
    expect(large?.textContent).not.toMatch(/\d+ (settled|still open|points?)/);
    expect(large?.querySelector(".wiki-working-on-when")).not.toBeNull();
    expect(document.querySelector('[data-thing-id="thg_2"] .wiki-working-on-when')).not.toBeNull();
  });

  it("UX-CHAT-5: says nothing about steeping that was not today", async () => {
    vi.mocked(listThings).mockResolvedValue([thing({ dreamed_at: now - 86_400 * 3 })]);
    renderWorkingOn();
    await screen.findByText("New tagline");
    expect(screen.queryByTestId("wiki-working-on-steeped")).toBeNull();
  });
});
