// SPDX-License-Identifier: AGPL-3.0-only
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { answerThingMapProposal, answerThingMerge, getMemorySaid, getThing, undoThingDream, getThingMap, listThings, moveSessionThing, renameThing, undoThingTidy, type ThingDetail as ThingRecord, type ThingMap } from "../../../lib/tauri";
import { ThingDetail } from "./ThingDetail";

vi.mock("../../../lib/tauri", async (importOriginal) => ({
  ...(await importOriginal<typeof import("../../../lib/tauri")>()),
  getThing: vi.fn(),
  getThingMap: vi.fn(),
  listThings: vi.fn(),
  renameThing: vi.fn(),
  moveSessionThing: vi.fn(),
  answerThingMerge: vi.fn(),
  undoThingDream: vi.fn(),
  answerThingMapProposal: vi.fn(),
  getMemorySaid: vi.fn(),
  undoThingTidy: vi.fn(),
}));

const now = Math.floor(Date.now() / 1000);

const tagline: ThingRecord = {
  id: "thg_1",
  title: "New tagline",
  title_by: "ai",
  status_line: "Picked the X wording; the site still shows the old line.",
  spaces: ["wenlan"],
  last_activity_at: now - 600,
  sessions: [{ id: "ses_a", tool: "claude-code", title: "Rewrite the hero line", last_write_at: now - 600 }],
  pages: [{ id: "page-1", title: "Tagline options", via: "wrote", at: now - 600 }],
  memories: [
    { source_id: "mem_2", title: "The hero line stays under eight words", memory_type: "preference", created_at: now - 300 },
    { source_id: "mem_1", title: "Lead with the X wording.", memory_type: "decision", created_at: now - 900 },
  ],
};

function renderThing(onOpenPage = vi.fn(), onOpenThing = vi.fn(), onOpenGraph?: (thingId: string) => void) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  render(
    <QueryClientProvider client={client}>
      <ThingDetail thingId="thg_1" onBack={vi.fn()} onOpenPage={onOpenPage} onOpenThing={onOpenThing} onOpenGraph={onOpenGraph} />
    </QueryClientProvider>,
  );
  return { onOpenPage, onOpenThing };
}

describe("ThingDetail", () => {
  beforeEach(() => {
    localStorage.clear();
    vi.mocked(getThing).mockReset().mockResolvedValue(tagline);
    vi.mocked(getThingMap).mockReset().mockResolvedValue({ thing_id: "thg_1", revision: 0, nodes: [], edges: [] });
    vi.mocked(listThings).mockReset().mockResolvedValue([
      tagline,
      { ...tagline, id: "thg_2", title: "Product Hunt launch" },
    ]);
    vi.mocked(renameThing).mockReset();
    vi.mocked(moveSessionThing).mockReset();
    vi.mocked(answerThingMerge).mockReset().mockResolvedValue(tagline);
  });

  it("UX-CHAT-5: shows where the thing stands, its conversations and pages, with no done state", async () => {
    const { onOpenPage } = renderThing();
    expect(await screen.findByRole("heading", { level: 1, name: "New tagline" })).toBeTruthy();
    expect(screen.getByText(/Picked the X wording/)).toBeTruthy();
    expect(screen.getByText("Rewrite the hero line")).toBeTruthy();
    expect(screen.queryByRole("button", { name: /done|wrap up|archive/i })).toBeNull();
    // The tool shows by its mark, as on Home; a page says the AI wrote it.
    expect(screen.getByRole("img", { name: "Claude Code" })).toBeTruthy();
    expect(screen.getByRole("button", { name: /^Tagline options/ })).toHaveTextContent("AI wrote");
    fireEvent.click(screen.getByRole("button", { name: /^Tagline options/ }));
    expect(onOpenPage).toHaveBeenCalledWith("page-1");
  });

  it("UX-VIEW-6: the short text is a recap in a fixed order: where it stands, decided, background", async () => {
    vi.mocked(getThing).mockResolvedValue({
      ...tagline,
      memories: [
        ...(tagline.memories ?? []),
        { source_id: "mem_3", title: "The old line still ships in the PH gallery", memory_type: "gotcha", created_at: now - 120 },
      ],
    });
    renderThing();
    const short = await screen.findByTestId("thing-short");
    await within(short).findByRole("region", { name: "Decided" });
    expect(short.firstElementChild).toHaveTextContent(/Picked the X wording/);
    expect(within(short).getAllByRole("region").map((region) => region.getAttribute("aria-label"))).toEqual(["Decided", "Background"]);
    expect(within(short).getByRole("region", { name: "Background" })).toHaveTextContent("The old line still ships in the PH gallery.");
  });

  it("UX-WHO-6: opening a point shows the lines of conversation it came from", async () => {
    vi.mocked(getMemorySaid).mockReset().mockResolvedValue({
      session_id: "ses_a",
      tool: "claude-code",
      lines: [{ index: 4, line: { speaker: "you", text: "Let's lead with the X wording.", at: now - 950 } }],
    });
    renderThing();
    const decided = await screen.findByRole("region", { name: "Decided" });
    const point = within(decided).getByRole("button", { name: "Lead with the X wording." });
    expect(point).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(point);
    expect(await within(decided).findByText("Let's lead with the X wording.")).toBeTruthy();
    expect(getMemorySaid).toHaveBeenCalledWith("mem_1");
    fireEvent.click(point);
    expect(within(decided).queryByText("Let's lead with the X wording.")).toBeNull();
  });

  it("UX-GRAPH-7: an opened point says what a night's tidy took the place of, and Undo puts it back", async () => {
    vi.mocked(getMemorySaid).mockReset().mockResolvedValue({ session_id: "ses_a", tool: "claude-code", lines: [] });
    vi.mocked(getThing).mockResolvedValue({
      ...tagline,
      tidied: [{ kind: "merge", memory_id: "mem_1", replaced: [
        { source_id: "mem_a", text: "Use the X wording first." },
        { source_id: "mem_b", text: "Lead with X." },
      ] }],
    });
    vi.mocked(undoThingTidy).mockReset().mockResolvedValue(tagline);
    renderThing();
    const decided = await screen.findByRole("region", { name: "Decided" });
    // Nothing inline until the point is opened.
    expect(within(decided).queryByText(/Took the place of/)).toBeNull();
    fireEvent.click(within(decided).getByRole("button", { name: "Lead with the X wording." }));
    const tidied = await within(decided).findByRole("group", { name: "Tidied" });
    expect(tidied).toHaveTextContent("Took the place of: Use the X wording first.");
    expect(tidied).toHaveTextContent("Took the place of: Lead with X.");
    fireEvent.click(within(tidied).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(undoThingTidy).toHaveBeenCalledWith("thg_1", "mem_1"));
  });

  it("UX-VIEW-6: the page opens with what the work has come to, then how it got there", async () => {
    vi.mocked(getThing).mockResolvedValue({
      ...tagline,
      dream: {
        at: now - 3_600,
        decided: 1,
        open: 0,
        conclusion: [
          { text: "The tagline leads with the X wording", points: ["mem_1"] },
          { text: "The hero line stays short.", points: ["mem_2"] },
        ],
      },
    });
    renderThing();
    const short = await screen.findByTestId("thing-short");
    await within(short).findByRole("region", { name: "Decided" });
    expect(short.firstElementChild).toHaveTextContent("The tagline leads with the X wording. The hero line stays short.");
    // The conclusion takes the place of where it stands; nothing else is added.
    expect(within(short).queryByText(/Picked the X wording/)).toBeNull();
    expect(within(short).getAllByTestId("thing-lead")).toHaveLength(1);
  });

  it("UX-VIEW-6: until a night writes a conclusion, the page opens with where it stands", async () => {
    vi.mocked(getThing).mockResolvedValue({ ...tagline, dream: { at: now - 3_600, decided: 1, open: 0, conclusion: [] } });
    renderThing();
    const short = await screen.findByTestId("thing-short");
    await within(short).findByRole("region", { name: "Decided" });
    expect(screen.getByTestId("thing-lead")).toHaveTextContent(/Picked the X wording/);
  });

  it("UX-VIEW-1: short text and mind map show the same points, and the last view is remembered", async () => {
    const { onOpenPage } = renderThing();
    const short = await screen.findByTestId("thing-short");
    expect(await within(short).findByRole("region", { name: "Decided" })).toHaveTextContent("Lead with the X wording.");
    expect(within(short).getByRole("region", { name: "Background" })).toHaveTextContent("The hero line stays under eight words.");
    fireEvent.click(screen.getByRole("button", { name: "Mind map" }));
    const map = screen.getByTestId("thing-map");
    expect(map).toHaveTextContent("Lead with the X wording.");
    expect(screen.queryByTestId("thing-short")).toBeNull();
    fireEvent.click(within(map).getByRole("button", { name: "Tagline options" }));
    expect(onOpenPage).toHaveBeenCalledWith("page-1");
    cleanup();
    renderThing();
    expect(await screen.findByTestId("thing-map")).toBeTruthy();
    expect(screen.getByRole("button", { name: "Mind map" })).toHaveAttribute("aria-pressed", "true");
  });

  it("shows the newest three points of a kind and folds the rest behind one line", async () => {
    vi.mocked(getThing).mockResolvedValue({
      ...tagline,
      memories: [1, 2, 3, 4, 5].map((n) => ({ source_id: `mem_${n}`, title: `Decision ${n}`, memory_type: "decision", created_at: now - n * 60 })),
    });
    renderThing();
    const decided = await screen.findByRole("region", { name: "Decided" });
    expect(within(decided).getAllByRole("listitem").map((item) => item.textContent)).toEqual(["Decision 1.", "Decision 2.", "Decision 3."]);
    fireEvent.click(within(decided).getByRole("button", { name: "2 more" }));
    expect(within(decided).getAllByRole("listitem")).toHaveLength(5);
  });

  it("UX-VIEW-7: once the AI writes the map, both views follow its shape", async () => {
    const node = (id: string, parent: string | null, kind: string, label: string, state?: "open" | "decided" | "dropped", rank = 0) =>
      ({ id, parent_id: parent, rank, kind, label, state, by: "ai" as const, sources: [] });
    const lisbon: ThingMap = {
      thing_id: "thg_1",
      revision: 4,
      edges: [],
      nodes: [
        node("root", null, "topic", "Plan the Lisbon trip"),
        node("q1", "root", "question", "Where to stay?", undefined, 1),
        node("o1", "q1", "option", "Alfama", "decided", 1),
        node("r1", "o1", "reason", "Old town on foot", undefined, 1),
        node("o2", "q1", "option", "Baixa", "dropped", 2),
        node("r2", "o2", "reason", "Too loud", undefined, 1),
        node("q2", "root", "question", "Where on the 14th?", "open", 2),
        node("o3", "q2", "option", "Sintra", undefined, 1),
        node("r3", "o3", "reason", "Pena Palace is quiet in the morning", undefined, 1),
        node("x1", "root", "a_future_kind", "Pack light", undefined, 3),
      ],
    };
    vi.mocked(getThingMap).mockResolvedValue(lisbon);
    renderThing();
    const decided = await screen.findByRole("region", { name: "Decided" });
    expect(decided).toHaveTextContent("Where to stay? Alfama.");
    expect(screen.getByRole("region", { name: "Still open" })).toHaveTextContent("Where on the 14th?");
    const background = screen.getByRole("region", { name: "Background" });
    expect(background).toHaveTextContent("Alfama Old town on foot.");
    expect(background).toHaveTextContent("Pack light.");
    // What was dropped stays on the map but out of the recap.
    expect(screen.getByTestId("thing-short")).not.toHaveTextContent("Too loud");
    expect(within(screen.getByTestId("thing-short")).getAllByRole("region").map((region) => region.getAttribute("aria-label"))).toEqual([
      "Decided",
      "Still open",
      "Background",
    ]);
    fireEvent.click(screen.getByRole("button", { name: "Mind map" }));
    const map = screen.getByTestId("thing-map");
    expect(map).toHaveTextContent("Too loud");
    expect(map.querySelector('[data-kind="question"]')).not.toBeNull();
    expect(map.querySelector('[data-state="dropped"]')).toHaveTextContent("Baixa");
    // A kind the app does not know is drawn as a plain point.
    expect(map.querySelector('[data-kind="plain"]')).toHaveTextContent("Pack light");
  });

  it("UX-THING-1: renames the thing in one step", async () => {
    vi.mocked(renameThing).mockResolvedValue({ ...tagline, title: "Tagline", title_by: "person" });
    renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Rename" }));
    const input = screen.getByRole("textbox", { name: "Name" });
    fireEvent.change(input, { target: { value: "Tagline" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await waitFor(() => expect(renameThing).toHaveBeenCalledWith("thg_1", "Tagline"));
  });

  it("UX-THING-1: moves a conversation to another thing in one step", async () => {
    vi.mocked(getThing).mockResolvedValue({
      ...tagline,
      sessions: [...tagline.sessions, { id: "ses_b", tool: "codex", title: "Shorter variants", last_write_at: now - 900 }],
    });
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_2", title: "Product Hunt launch" });
    const { onOpenThing } = renderThing();
    fireEvent.click((await screen.findAllByRole("button", { name: "Move" }))[0]);
    fireEvent.click(await screen.findByRole("button", { name: "Product Hunt launch" }));
    await waitFor(() => expect(moveSessionThing).toHaveBeenCalledWith("ses_a", "thg_1", { thingId: "thg_2" }));
    fireEvent.click(await screen.findByRole("button", { name: "Open" }));
    expect(onOpenThing).toHaveBeenCalledWith("thg_2");
  });

  it("UX-CHAT-5: moving the last conversation out goes along with it", async () => {
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_2", title: "Product Hunt launch" });
    const { onOpenThing } = renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Move" }));
    fireEvent.click(await screen.findByRole("button", { name: "Product Hunt launch" }));
    await waitFor(() => expect(onOpenThing).toHaveBeenCalledWith("thg_2"));
  });

  it("UX-THING-1: moves a conversation into a new thing it names", async () => {
    vi.mocked(moveSessionThing).mockResolvedValue({ ...tagline, id: "thg_3", title: "Hero copy" });
    renderThing();
    fireEvent.click(await screen.findByRole("button", { name: "Move" }));
    fireEvent.change(screen.getByRole("textbox", { name: "New name" }), { target: { value: "Hero copy" } });
    fireEvent.click(screen.getByRole("button", { name: "Start new" }));
    await waitFor(() => expect(moveSessionThing).toHaveBeenCalledWith("ses_a", "thg_1", { newTitle: "Hero copy" }));
  });

  it("UX-THING-3: two things that look alike are only a proposal; they merge when you press it", async () => {
    vi.mocked(getThing).mockResolvedValue({ ...tagline, merge_proposals: [{ id: "thg_2", title: "Product Hunt launch" }] });
    renderThing();
    const line = await screen.findByRole("group", { name: "Looks like the same thing as “Product Hunt launch”." });
    // Nothing merged by itself: the page still shows its own thing.
    expect(answerThingMerge).not.toHaveBeenCalled();
    expect(screen.getByRole("heading", { level: 1, name: "New tagline" })).toBeTruthy();
    vi.mocked(getThing).mockResolvedValue(tagline);
    fireEvent.click(within(line).getByRole("button", { name: "Merge" }));
    await waitFor(() => expect(answerThingMerge).toHaveBeenCalledWith("thg_1", "thg_2", true));
    await waitFor(() => expect(screen.queryByRole("group", { name: /Looks like the same thing/ })).toBeNull());
  });

  it("UX-THING-3: keep apart answers the proposal without merging", async () => {
    vi.mocked(getThing).mockResolvedValue({ ...tagline, merge_proposals: [{ id: "thg_2", title: "Product Hunt launch" }] });
    renderThing();
    const line = await screen.findByRole("group", { name: /Looks like the same thing/ });
    fireEvent.click(within(line).getByRole("button", { name: "Keep apart" }));
    await waitFor(() => expect(answerThingMerge).toHaveBeenCalledWith("thg_1", "thg_2", false));
  });

  it("UX-THING-3: no proposal, no line", async () => {
    renderThing();
    await screen.findByRole("heading", { level: 1, name: "New tagline" });
    expect(screen.queryByRole("group", { name: /Looks like the same thing/ })).toBeNull();
  });

  it("UX-CHAT-5: the page says what last night steeped, groups what is still open, marks the new points and can take them back", async () => {
    const steeped: ThingRecord = {
      ...tagline,
      memories: [
        ...(tagline.memories ?? []),
        { source_id: "mem_3", title: "Whether the site gets a pricing page", memory_type: "fact", created_at: now - 60 },
      ],
      dream: { at: now - 60, decided: 1, open: 1, fresh: ["mem_1", "mem_3"] },
      open_points: ["mem_3"],
    };
    vi.mocked(getThing).mockResolvedValue(steeped);
    vi.mocked(undoThingDream).mockResolvedValue(tagline);
    renderThing();
    const line = await screen.findByRole("group", { name: "Steeped 2 points overnight" });
    const open = await screen.findByRole("region", { name: "Still open" });
    expect(within(open).getByText(/pricing page/)).toBeTruthy();
    expect(within(open).getByText(/pricing page/).closest("li")).toHaveAttribute("data-fresh", "true");
    const decided = screen.getByRole("region", { name: "Decided" });
    expect(within(decided).getByText(/X wording/).closest("li")).toHaveAttribute("data-fresh", "true");
    const background = screen.getByRole("region", { name: "Background" });
    expect(within(background).getByText(/eight words/).closest("li")).not.toHaveAttribute("data-fresh");
    vi.mocked(getThing).mockResolvedValue(tagline);
    fireEvent.click(within(line).getByRole("button", { name: "Undo" }));
    await waitFor(() => expect(undoThingDream).toHaveBeenCalledWith("thg_1"));
    await waitFor(() => expect(screen.queryByRole("group", { name: /overnight/ })).toBeNull());
  });

  it("UX-CHAT-5: a conversation a night could not read is said once, quietly", async () => {
    vi.mocked(getThing).mockResolvedValue({ ...tagline, unread: 1 });
    renderThing();
    expect(await screen.findByText("One conversation couldn't be read overnight.")).toBeTruthy();
    cleanup();
    vi.mocked(getThing).mockResolvedValue(tagline);
    renderThing();
    await screen.findByRole("heading", { level: 1, name: "New tagline" });
    expect(screen.queryByText(/couldn't be read/)).toBeNull();
  });

  it("UX-CHAT-5: a steeped thing opens on the big map from its night's line", async () => {
    vi.mocked(getThing).mockResolvedValue({ ...tagline, dream: { at: now - 60, decided: 1, open: 0, fresh: ["mem_1"] } });
    const onOpenGraph = vi.fn();
    renderThing(vi.fn(), vi.fn(), onOpenGraph);
    const line = await screen.findByRole("group", { name: /overnight/ });
    fireEvent.click(within(line).getByRole("button", { name: "On the big map" }));
    expect(onOpenGraph).toHaveBeenCalledWith("thg_1");
  });

  it("UX-CHAT-5: an older night is dated, and nothing is marked new", async () => {
    const at = now - 86_400 * 3;
    vi.mocked(getThing).mockResolvedValue({ ...tagline, dream: { at, decided: 1, open: 0, fresh: ["mem_1"] } });
    renderThing();
    await screen.findByRole("group", { name: /^Steeped 1 point on / });
    expect(document.querySelector("li[data-fresh]")).toBeNull();
  });

  it("UX-CHAT-5: on a map the AI drew, the points last night steeped are marked, and the thing's other points still show", async () => {
    const steeped: ThingRecord = {
      ...tagline,
      memories: [
        ...(tagline.memories ?? []),
        { source_id: "mem_3", title: "No pricing page for now", memory_type: "decision", created_at: now - 60 },
      ],
      dream: { at: now - 60, decided: 1, open: 0, fresh: ["mem_3"] },
    };
    vi.mocked(getThing).mockResolvedValue(steeped);
    vi.mocked(getThingMap).mockResolvedValue({
      thing_id: "thg_1",
      revision: 3,
      edges: [],
      nodes: [
        { id: "root", parent_id: null, rank: 0, kind: "topic", label: "New tagline", by: "ai", sources: [] },
        {
          id: "p1", parent_id: "root", rank: 1, kind: "question", state: "decided", label: "Pricing page?", by: "ai",
          sources: [{ session_id: "ses_a", memory_id: "mem_3" }],
        },
      ],
    });
    renderThing();
    const decided = await screen.findByRole("region", { name: "Decided" });
    const point = within(decided).getByText(/Pricing page\?/).closest("li");
    expect(point).toHaveAttribute("data-fresh", "true");
    // One moon per thing, on the line above; none on each point.
    expect(within(point as HTMLElement).queryByRole("img", { name: "Steeped overnight" })).toBeNull();
    // The memory the map holds shows once, as the map's point.
    expect(screen.getByTestId("thing-short")).not.toHaveTextContent("No pricing page for now");
    // Points of the thing that no map point holds join their group.
    expect(decided).toHaveTextContent("Lead with the X wording.");
    expect(screen.getByRole("region", { name: "Background" })).toHaveTextContent("The hero line stays under eight words.");
    fireEvent.click(screen.getByRole("button", { name: "Mind map" }));
    expect(screen.getByTestId("thing-map").querySelector('[data-fresh="true"]')).toHaveTextContent("Pricing page?");
  });

  it("UX-WHO-2: what a night read on a point you wrote waits under it, and you answer it there", async () => {
    vi.mocked(getThingMap).mockResolvedValue({
      thing_id: "thg_1",
      revision: 5,
      edges: [],
      nodes: [
        { id: "root", parent_id: null, rank: 0, kind: "topic", label: "New tagline", by: "ai", sources: [] },
        { id: "p1", parent_id: "root", rank: 1, kind: "point", state: "open", label: "Pricing page?", by: "you", sources: [] },
      ],
      proposals: [
        { id: "tmp_1", by: "local", change: { op: "edit", id: "p1", label: "No pricing page for now.", state: "decided" }, created_at: now - 60 },
      ],
    });
    vi.mocked(answerThingMapProposal).mockResolvedValue({ thing_id: "thg_1", revision: 6, nodes: [], edges: [] });
    renderThing();
    const row = await screen.findByTestId("thing-recap-proposal");
    expect(row).toHaveTextContent("Read overnight: No pricing page for now.");
    expect(row.closest("li")).toHaveTextContent("Pricing page?");
    fireEvent.click(within(row).getByRole("button", { name: "Use this" }));
    await waitFor(() => expect(answerThingMapProposal).toHaveBeenCalledWith("thg_1", 5, "tmp_1", true));
  });
});
